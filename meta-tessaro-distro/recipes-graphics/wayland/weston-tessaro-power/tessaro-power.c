/*
 * tessaro-power.so: switch Weston's outputs off and on from outside.
 *
 * Weston 13 can power an output down (weston_output_power_off: the CRTC is
 * disabled, the signal really stops) but exposes it to nobody - no protocol,
 * no D-Bus, no signal. This module listens on a unix socket in Weston's
 * runtime directory and takes one line per connection:
 *
 *   off     every output off, and every output that appears later
 *   on      every output back on
 *   status  nothing changes
 *
 * and answers with the state afterwards, "on\n" or "off\n", or "error: ...\n".
 * Touch and keys do not wake an output powered off this way: that is the
 * point of the forced state over weston_compositor_sleep().
 *
 * The state lives as long as Weston does. tessaro-agent keeps what it wants
 * in /run/tessaro-kiosk and puts it back after Weston restarts; see
 * docs/display.md, "Screen power".
 */

#define _GNU_SOURCE /* accept4 */

#include <errno.h>
#include <fcntl.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

#include <wayland-server.h>
#include <libweston/libweston.h>
#include <weston.h>

#define SOCKET_NAME "power.sock"
#define LINE_MAX_BYTES 64

struct power {
	struct weston_compositor *compositor;
	bool off;
	int listen_fd;
	char path[108];
	struct wl_event_source *listen_source;
	struct wl_listener output_created;
	struct wl_listener destroy;
};

struct client {
	struct power *power;
	int fd;
	struct wl_event_source *source;
	char line[LINE_MAX_BYTES];
	size_t len;
};

static void
apply(struct power *power)
{
	struct weston_output *output;

	wl_list_for_each(output, &power->compositor->output_list, link) {
		if (power->off)
			weston_output_power_off(output);
		else
			weston_output_power_on(output);
	}
}

/* A TV that drops hot-plug detection in standby comes back as a new output,
 * which starts powered on. */
static void
output_created(struct wl_listener *listener, void *data)
{
	struct power *power = wl_container_of(listener, power, output_created);
	struct weston_output *output = data;

	if (power->off)
		weston_output_power_off(output);
}

static void
client_close(struct client *client)
{
	wl_event_source_remove(client->source);
	close(client->fd);
	free(client);
}

static void
answer(struct client *client, const char *text)
{
	/* A few bytes to a fresh socket: the send buffer takes them whole, and a
	 * client that is gone only costs the answer. */
	ssize_t written = send(client->fd, text, strlen(text), MSG_NOSIGNAL);
	(void)written;
}

static void
handle(struct client *client, const char *command)
{
	struct power *power = client->power;

	if (strcmp(command, "off") == 0) {
		if (!power->off)
			weston_log("tessaro-power: outputs off\n");
		power->off = true;
		apply(power);
	} else if (strcmp(command, "on") == 0) {
		if (power->off)
			weston_log("tessaro-power: outputs on\n");
		power->off = false;
		apply(power);
	} else if (strcmp(command, "status") != 0) {
		answer(client, "error: unknown command; send on, off or status\n");
		return;
	}
	answer(client, power->off ? "off\n" : "on\n");
}

static int
client_readable(int fd, uint32_t mask, void *data)
{
	struct client *client = data;
	char *newline;
	ssize_t got;

	if (mask & (WL_EVENT_HANGUP | WL_EVENT_ERROR)) {
		client_close(client);
		return 0;
	}

	got = read(fd, client->line + client->len,
		   sizeof client->line - 1 - client->len);
	if (got < 0 && (errno == EAGAIN || errno == EINTR))
		return 0;
	if (got <= 0) {
		client_close(client);
		return 0;
	}
	client->len += got;
	client->line[client->len] = '\0';

	newline = strchr(client->line, '\n');
	if (newline) {
		*newline = '\0';
		if (newline > client->line && newline[-1] == '\r')
			newline[-1] = '\0';
		handle(client, client->line);
		client_close(client);
	} else if (client->len == sizeof client->line - 1) {
		answer(client, "error: line too long\n");
		client_close(client);
	}
	return 0;
}

static int
listen_readable(int fd, uint32_t mask, void *data)
{
	struct power *power = data;
	struct wl_event_loop *loop;
	struct client *client;
	int accepted;

	accepted = accept4(fd, NULL, NULL, SOCK_CLOEXEC | SOCK_NONBLOCK);
	if (accepted < 0)
		return 0;

	client = calloc(1, sizeof *client);
	if (!client) {
		close(accepted);
		return 0;
	}
	client->power = power;
	client->fd = accepted;

	loop = wl_display_get_event_loop(power->compositor->wl_display);
	client->source = wl_event_loop_add_fd(loop, accepted, WL_EVENT_READABLE,
					      client_readable, client);
	if (!client->source) {
		close(accepted);
		free(client);
	}
	return 0;
}

static void
power_destroy(struct wl_listener *listener, void *data)
{
	struct power *power = wl_container_of(listener, power, destroy);

	wl_list_remove(&power->output_created.link);
	wl_list_remove(&power->destroy.link);
	if (power->listen_source)
		wl_event_source_remove(power->listen_source);
	if (power->listen_fd >= 0) {
		close(power->listen_fd);
		unlink(power->path);
	}
	free(power);
}

static int
open_socket(struct power *power)
{
	const char *dir = getenv("RUNTIME_DIRECTORY");
	struct sockaddr_un address = { .sun_family = AF_UNIX };
	int fd;

	/* systemd's RuntimeDirectory=weston; the fallback is its default path. */
	if (!dir || !*dir)
		dir = "/run/weston";
	if (snprintf(power->path, sizeof power->path, "%s/%s", dir, SOCKET_NAME) >=
	    (int)sizeof power->path)
		return -1;
	memcpy(address.sun_path, power->path, sizeof address.sun_path);

	fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
	if (fd < 0)
		return -1;
	unlink(power->path);
	/* The owner only: Weston's user, and root, which tessaro-agent runs as. */
	if (bind(fd, (struct sockaddr *)&address, sizeof address) < 0 ||
	    chmod(power->path, 0600) < 0 || listen(fd, 4) < 0) {
		close(fd);
		return -1;
	}
	return fd;
}

WL_EXPORT int
wet_module_init(struct weston_compositor *compositor, int *argc, char *argv[])
{
	struct wl_event_loop *loop = wl_display_get_event_loop(compositor->wl_display);
	struct power *power;

	power = calloc(1, sizeof *power);
	if (!power)
		return -1;
	power->compositor = compositor;

	power->listen_fd = open_socket(power);
	if (power->listen_fd < 0) {
		weston_log("tessaro-power: cannot listen on %s: %s\n", power->path,
			   strerror(errno));
		free(power);
		/* Not fatal: the display works, it only cannot be switched off. */
		return 0;
	}
	power->listen_source = wl_event_loop_add_fd(loop, power->listen_fd,
						    WL_EVENT_READABLE,
						    listen_readable, power);

	power->output_created.notify = output_created;
	wl_signal_add(&compositor->output_created_signal, &power->output_created);
	power->destroy.notify = power_destroy;
	wl_signal_add(&compositor->destroy_signal, &power->destroy);

	weston_log("tessaro-power: listening on %s\n", power->path);
	return 0;
}
