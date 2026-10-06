//! `tessaro-ctl playlist ...`: playlists of pages, images and videos the
//! player page shows in turn instead of browser.url, and the timetable that
//! picks which one plays when.
//!
//! The device keeps the playlists and the timetable; playlist.default names
//! the one that plays when no timetable entry covers now. With neither, the
//! browser shows browser.url directly.

use std::path::PathBuf;

use crate::out::println;
use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Args, Subcommand};
use protocol::api::{self, PlaylistItemMoveBody, PlaylistItemRef, PlaylistRef, TimetableRef};
use protocol::playlist::{Fit, PlaylistInfo, TimetableInfo, Transition, PLAYLIST_TRANSITION_MAX};
use tessaro_client::describe::playlist as describe;
use tessaro_client::playlist::{self as shared, EntryFields, ItemFields};
use tessaro_client::schedule::now;
use tessaro_client::text::Line;

use crate::connect::Session;
use crate::style::{self, paint};
use crate::{done, print, prompt};

#[derive(Subcommand)]
pub enum PlaylistCmd {
    /// Every playlist: how many items, the one on screen, which is
    /// playlist.default and which the timetable plays.
    List,
    /// One playlist and its items.
    Show { playlist: String },
    /// Add a playlist, empty or from a file.
    ///
    ///   tessaro-ctl playlist create lobby
    ///   tessaro-ctl playlist create lunch --transition slide --transition-ms 500
    ///   tessaro-ctl playlist create lobby --file lobby.json
    Create {
        /// Lower-case letters, digits and -.
        name: String,
        #[command(flatten)]
        look: Look,
        /// Items and transition from a JSON file: what `tessaro-ctl
        /// playlist show --json` prints, or a list of items.
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
    },
    /// Change a playlist, by name or id: rename it, its transition, or all
    /// of its items from a file.
    ///
    ///   tessaro-ctl playlist set lobby --name entrance
    ///   tessaro-ctl playlist show lobby --json > lobby.json; tessaro-ctl playlist set lobby --file lobby.json
    Set {
        playlist: String,
        /// Rename it.
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        look: Look,
        /// The file's items replace them all, and its transition the
        /// playlist's unless one is given.
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
    },
    /// Remove a playlist that is neither playlist.default nor in the
    /// timetable.
    Remove {
        playlist: String,
        #[arg(long, short)]
        yes: bool,
    },
    /// What the player is doing: on screen or not, the playlist and why,
    /// the item on screen, what it skipped, how far the media copies are.
    Status,
    /// A playlist's items: add, change, remove and move them.
    #[command(subcommand)]
    Items(Box<ItemsCmd>),
    /// When a playlist plays instead of playlist.default.
    #[command(subcommand)]
    Timetable(TimetableCmd),
}

/// How a playlist's items replace each other.
#[derive(Args)]
pub struct Look {
    /// How one item replaces the one before.
    #[arg(long, value_parser = transition_parser())]
    transition: Option<Transition>,
    /// How long the transition takes, milliseconds.
    #[arg(long, value_name = "MS",
        value_parser = clap::value_parser!(u32).range(0..=PLAYLIST_TRANSITION_MAX as i64))]
    transition_ms: Option<u32>,
}

fn transition_parser() -> impl TypedValueParser<Value = Transition> {
    PossibleValuesParser::new(Transition::NAMES)
        .map(|name| name.parse::<Transition>().expect("one of the names"))
}

#[derive(Subcommand)]
pub enum ItemsCmd {
    /// Add an item: a page, an image or a video, by URL. Files in the
    /// store are http://127.0.0.1/files/...; other images and videos are
    /// kept on the device to play offline.
    ///
    ///   tessaro-ctl playlist items add lobby --image http://127.0.0.1/files/welcome.png --duration 10s
    ///   tessaro-ctl playlist items add lobby --video https://cdn.test/promo.mp4 --from 5s --to 0:30 --sound --volume 60
    ///   tessaro-ctl playlist items add lunch --url https://menu.test/ --duration 1m --interactive --idle 30s --at 1
    Add {
        playlist: String,
        #[command(flatten)]
        source: Source,
        #[command(flatten)]
        options: ItemOptions,
        /// Make it interactive: touch and keys hold it on screen.
        #[arg(long)]
        interactive: bool,
        /// Play a video's sound; muted otherwise.
        #[arg(long)]
        sound: bool,
        /// Give a URL item the page bridge, window.tessaro.
        #[arg(long)]
        bridge: bool,
        /// Its place, from 1; at the end without it.
        #[arg(long, value_name = "POS", value_parser = clap::value_parser!(u32).range(1..))]
        at: Option<u32>,
    },
    /// Change an item, by its place from 1: only what is given changes. An
    /// empty value ('') puts an option back to its default.
    ///
    ///   tessaro-ctl playlist items set lobby 2 --duration 20s
    ///   tessaro-ctl playlist items set lobby 3 --no-sound --from ''
    Set {
        playlist: String,
        #[arg(value_parser = clap::value_parser!(u32).range(1..))]
        position: u32,
        /// A new URL for it.
        #[arg(long, value_name = "URL")]
        src: Option<String>,
        #[command(flatten)]
        options: ItemOptions,
        #[arg(long, conflicts_with = "no_interactive")]
        interactive: bool,
        #[arg(long)]
        no_interactive: bool,
        #[arg(long, conflicts_with = "no_sound")]
        sound: bool,
        #[arg(long)]
        no_sound: bool,
        #[arg(long, conflicts_with = "no_bridge")]
        bridge: bool,
        #[arg(long)]
        no_bridge: bool,
    },
    /// Remove an item, by its place from 1.
    Remove {
        playlist: String,
        #[arg(value_parser = clap::value_parser!(u32).range(1..))]
        position: u32,
        #[arg(long, short)]
        yes: bool,
    },
    /// Move an item from one place to another, both from 1.
    ///
    ///   tessaro-ctl playlist items move lobby 4 1
    Move {
        playlist: String,
        #[arg(value_parser = clap::value_parser!(u32).range(1..))]
        position: u32,
        #[arg(value_parser = clap::value_parser!(u32).range(1..))]
        to: u32,
    },
}

/// What an item shows.
#[derive(Args)]
#[group(required = true, multiple = false)]
pub struct Source {
    /// A web page.
    #[arg(long, value_name = "URL")]
    url: Option<String>,
    /// A picture.
    #[arg(long, value_name = "URL")]
    image: Option<String>,
    /// A video; it plays to its end or --to.
    #[arg(long, value_name = "URL")]
    video: Option<String>,
}

/// How an item plays, each as typed.
#[derive(Args)]
pub struct ItemOptions {
    /// How long a page or image stays on screen: 10s, 1m30s, 90. 10s
    /// when not given.
    #[arg(long, value_name = "TIME")]
    duration: Option<String>,
    /// Where a video starts: 5s, 1.5s, 0:05.
    #[arg(long, value_name = "TIME")]
    from: Option<String>,
    /// Where a video stops: 10s, 1:02.5.
    #[arg(long, value_name = "TIME")]
    to: Option<String>,
    /// A video's volume, 0-100; turns its sound on.
    #[arg(long, value_name = "N")]
    volume: Option<String>,
    /// How an image or video fills the screen.
    #[arg(long, value_parser = optional_names(Fit::NAMES))]
    fit: Option<String>,
    /// What shows around an image or video that does not fill the screen,
    /// #rrggbb.
    #[arg(long, value_name = "COLOR")]
    background: Option<String>,
    /// How this item comes on screen, instead of the playlist's.
    #[arg(long, value_parser = optional_names(Transition::NAMES))]
    transition: Option<String>,
    /// How long its transition takes, milliseconds.
    #[arg(long, value_name = "MS")]
    transition_ms: Option<String>,
    /// Seconds without input before an interactive item moves on: 30s.
    #[arg(long, value_name = "TIME")]
    idle: Option<String>,
    /// How long a page waits after it loads before it shows: 500ms.
    #[arg(long, value_name = "TIME")]
    ready_delay: Option<String>,
}

/// `names`, or empty for the default.
fn optional_names(names: &'static [&'static str]) -> PossibleValuesParser {
    PossibleValuesParser::new(names.iter().copied().chain([""]))
}

impl ItemOptions {
    fn fields(self) -> ItemFields {
        ItemFields {
            duration: self.duration,
            from: self.from,
            to: self.to,
            volume: self.volume,
            fit: self.fit,
            background: self.background,
            transition: self.transition,
            transition_ms: self.transition_ms,
            idle: self.idle,
            ready_delay: self.ready_delay,
            ..ItemFields::default()
        }
    }
}

#[derive(Subcommand)]
pub enum TimetableCmd {
    /// Every entry in order: its days and times, the playlist, its
    /// priority, the one deciding now marked.
    List,
    /// Add an entry: the playlist plays between the times on the days.
    ///
    ///   tessaro-ctl playlist timetable add lunch --from 11:30 --to 14:00 --days mon-fri
    ///   tessaro-ctl playlist timetable add night --from 22:00 --to 06:00 --priority 5
    Add {
        /// The playlist, by name or id.
        playlist: String,
        /// HH:MM, the device's time.
        #[arg(long, value_name = "HH:MM")]
        from: String,
        /// HH:MM. Earlier than --from runs past midnight; the same is the
        /// whole day.
        #[arg(long, value_name = "HH:MM")]
        to: String,
        /// The days it starts on: mon-fri, sat,sun, mon,wed-fri. Every day
        /// without it.
        #[arg(long, value_name = "DAYS")]
        days: Option<String>,
        /// Where entries overlap the highest wins, then the one listed
        /// first.
        #[arg(long, default_value_t = 0, allow_negative_numbers = true)]
        priority: i32,
        /// Save it switched off.
        #[arg(long)]
        disabled: bool,
    },
    /// Change an entry, by its id or the start of it: only what is given.
    ///
    ///   tessaro-ctl playlist timetable set 7f3a9c21 --to 15:00 --days all
    Set {
        entry: String,
        /// The playlist, by name or id.
        #[arg(long)]
        playlist: Option<String>,
        #[arg(long, value_name = "HH:MM")]
        from: Option<String>,
        #[arg(long, value_name = "HH:MM")]
        to: Option<String>,
        /// mon-fri, sat,sun; all for every day.
        #[arg(long, value_name = "DAYS")]
        days: Option<String>,
        #[arg(long, allow_negative_numbers = true)]
        priority: Option<i32>,
        #[arg(long, conflicts_with = "disable")]
        enable: bool,
        #[arg(long)]
        disable: bool,
    },
    /// Remove an entry, by its id or the start of it.
    Remove {
        entry: String,
        #[arg(long, short)]
        yes: bool,
    },
}

pub fn run(session: &mut Session, command: PlaylistCmd, json: bool) -> Result<(), String> {
    match command {
        PlaylistCmd::List => {
            let playlists = session.fetch::<api::playlist::List>()?;
            print(json, &playlists, || lines(describe::list(&playlists)))
        }
        PlaylistCmd::Show { playlist } => {
            let info = session.call::<api::playlist::Show>(PlaylistRef { playlist }, ())?;
            print(json, &info, || show(&info))
        }
        PlaylistCmd::Create { name, look, file } => {
            let file = file.as_deref().map(shared::read_file).transpose()?;
            let spec = shared::create_spec(&name, file, look.transition, look.transition_ms)?;
            let info = session.send::<api::playlist::Create>(spec)?;
            print(json, &info, || {
                println!(
                    "{}",
                    paint(style::OK, format!("created {}", info.spec.name))
                );
                show(&info);
            })
        }
        PlaylistCmd::Set {
            playlist,
            name,
            look,
            file,
        } => {
            let file = file.as_deref().map(shared::read_file).transpose()?;
            let change = shared::change(name, file, look.transition, look.transition_ms)?;
            let info = session.call::<api::playlist::Change>(PlaylistRef { playlist }, change)?;
            changed(&info, json)
        }
        PlaylistCmd::Remove { playlist, yes } => {
            prompt::confirm(yes, &format!("Remove playlist {playlist}?"))?;
            done::<api::playlist::Remove>(session, PlaylistRef { playlist }, (), json)
        }
        PlaylistCmd::Status => {
            let status = session.fetch::<api::playlist::Status>()?;
            print(json, &status, || {
                style::facts(&describe::status(&status, now()));
            })
        }
        PlaylistCmd::Items(command) => items(session, *command, json),
        PlaylistCmd::Timetable(command) => timetable(session, command, json),
    }
}

fn items(session: &mut Session, command: ItemsCmd, json: bool) -> Result<(), String> {
    match command {
        ItemsCmd::Add {
            playlist,
            source,
            options,
            interactive,
            sound,
            bridge,
            at,
        } => {
            let (kind, src) = match (source.url, source.image, source.video) {
                (Some(url), _, _) => ("url", url),
                (_, Some(image), _) => ("image", image),
                (_, _, Some(video)) => ("video", video),
                _ => return Err("give --url, --image or --video".to_string()),
            };
            let fields = ItemFields {
                kind: Some(kind.to_string()),
                src: Some(src),
                interactive: interactive.then_some(true),
                sound: sound.then_some(true),
                bridge: bridge.then_some(true),
                ..options.fields()
            };
            let info = shared::add_item(session, &playlist, &fields, at)?;
            changed(&info, json)
        }
        ItemsCmd::Set {
            playlist,
            position,
            src,
            options,
            interactive,
            no_interactive,
            sound,
            no_sound,
            bridge,
            no_bridge,
        } => {
            let flag = |on: bool, off: bool| match (on, off) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            };
            let fields = ItemFields {
                src,
                interactive: flag(interactive, no_interactive),
                sound: flag(sound, no_sound),
                bridge: flag(bridge, no_bridge),
                ..options.fields()
            };
            if fields == ItemFields::default() {
                return Err(
                    "nothing to change; `tessaro-ctl playlist items set --help` lists what can be"
                        .to_string(),
                );
            }
            let info = shared::set_item(session, &playlist, position, &fields)?;
            changed(&info, json)
        }
        ItemsCmd::Remove {
            playlist,
            position,
            yes,
        } => {
            prompt::confirm(yes, &format!("Remove item {position} of {playlist}?"))?;
            let info = session
                .call::<api::playlist::ItemRemove>(PlaylistItemRef { playlist, position }, ())?;
            changed(&info, json)
        }
        ItemsCmd::Move {
            playlist,
            position,
            to,
        } => {
            let info = session.call::<api::playlist::ItemMove>(
                PlaylistItemRef { playlist, position },
                PlaylistItemMoveBody { to },
            )?;
            changed(&info, json)
        }
    }
}

fn timetable(session: &mut Session, command: TimetableCmd, json: bool) -> Result<(), String> {
    match command {
        TimetableCmd::List => {
            let entries = session.fetch::<api::playlist::TimetableList>()?;
            print(json, &entries, || lines(describe::timetable(&entries)))
        }
        TimetableCmd::Add {
            playlist,
            from,
            to,
            days,
            priority,
            disabled,
        } => {
            let spec =
                shared::entry_spec(&playlist, days.as_deref(), &from, &to, priority, !disabled)?;
            let entry = session.send::<api::playlist::TimetableCreate>(spec)?;
            entry_changed("added", &entry, json)
        }
        TimetableCmd::Set {
            entry,
            playlist,
            from,
            to,
            days,
            priority,
            enable,
            disable,
        } => {
            let enabled = match (enable, disable) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            };
            let change = shared::entry_change(&EntryFields {
                playlist,
                days,
                from,
                to,
                priority,
                enabled,
            })?;
            let entry =
                session.call::<api::playlist::TimetableChange>(TimetableRef { entry }, change)?;
            entry_changed("changed", &entry, json)
        }
        TimetableCmd::Remove { entry, yes } => {
            prompt::confirm(yes, &format!("Remove timetable entry {entry}?"))?;
            done::<api::playlist::TimetableRemove>(session, TimetableRef { entry }, (), json)
        }
    }
}

/// A playlist in full: its facts, then its items.
fn show(info: &PlaylistInfo) {
    style::facts(&describe::show(info));
    println!();
    lines(describe::items(info));
}

/// What a change of a playlist or its items left.
fn changed(info: &PlaylistInfo, json: bool) -> Result<(), String> {
    print(json, info, || {
        println!(
            "{}",
            paint(style::OK, format!("changed {}", info.spec.name))
        );
        lines(describe::items(info));
    })
}

fn entry_changed(verb: &str, entry: &TimetableInfo, json: bool) -> Result<(), String> {
    print(json, entry, || {
        println!(
            "{}",
            paint(style::OK, format!("{verb} timetable entry {}", entry.id))
        );
        lines(vec![describe::entry(entry)]);
    })
}

fn lines(lines: Vec<Line>) {
    for line in lines {
        println!("{}", style::line(&line));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: PlaylistCmd,
    }

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(["playlist"].iter().chain(args))
    }

    #[test]
    fn an_item_is_one_kind_of_source() {
        assert!(parse(&["items", "add", "lobby", "--url", "https://a.test/"]).is_ok());
        assert!(parse(&["items", "add", "lobby"]).is_err());
        assert!(parse(&[
            "items",
            "add",
            "lobby",
            "--url",
            "https://a.test/",
            "--video",
            "https://a.test/a.mp4"
        ])
        .is_err());
    }

    #[test]
    fn a_flag_and_its_opposite_conflict() {
        assert!(parse(&["items", "set", "lobby", "2", "--no-sound"]).is_ok());
        assert!(parse(&["items", "set", "lobby", "2", "--sound", "--no-sound"]).is_err());
        assert!(parse(&["items", "set", "lobby", "0", "--sound"]).is_err());
        assert!(parse(&["items", "set", "lobby", "1", "--fit", ""]).is_ok());
        assert!(parse(&["items", "set", "lobby", "1", "--fit", "zoom"]).is_err());
        assert!(parse(&["timetable", "set", "abc", "--enable", "--disable"]).is_err());
    }

    #[test]
    fn a_priority_may_be_negative() {
        assert!(parse(&[
            "timetable",
            "add",
            "lobby",
            "--from",
            "00:00",
            "--to",
            "00:00",
            "--priority",
            "-1"
        ])
        .is_ok());
    }
}
