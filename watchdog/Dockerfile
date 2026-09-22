# The image that runs on the device: Ruby 4 with the watchdog and its gems.
# Built by `mise run watchdog-image`, which also exports it for the image
# recipe (meta-tessaro-distro/recipes-browser/tessaro-kiosk/files/).
FROM ruby:4.0-slim-bookworm

WORKDIR /app

COPY Gemfile Gemfile.lock ./
RUN bundle config set --local without test && bundle install

COPY lib ./lib
COPY bin ./bin

# An absolute interpreter path, not a PATH lookup: the unit runs this container
# with podman's --env-host, so the host's PATH replaces the image's and
# /usr/local/bundle/bin drops out of it. GEM_HOME still points at the bundle
# (systemd sets no such variable, so the image's value survives) and the
# watchdog shells out nowhere, so nothing else cares about PATH.
ENTRYPOINT ["/usr/local/bin/ruby", "bin/tessaro-kiosk-watchdog"]
