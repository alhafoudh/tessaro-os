//! One command on several devices: which devices a list of names and a set
//! of tags stand for, and running something on each of them, a few at a
//! time, with a result per device. `tessaro-ctl --node a,b` / `--tag` and
//! the GUI's bulk window both pick and run through here
//! (docs/clients.md, **Running on several devices**).
//!
//! The devices are worked out once, before anything runs: a name is
//! resolved as `connect::resolve` does, and at most one scan is made - only
//! when a tag is given or a name is not a known device's - and shared by
//! every name and tag. A device the scan found is reached at the address it
//! announced, so no device scans again when its session opens.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::connect::{self, Found, Target};
use crate::nodes::{Node, Nodes};
use crate::tags;

/// How many devices run at once, unless the caller says otherwise.
pub const PARALLEL: usize = 8;

/// One device a bulk run goes to.
#[derive(Debug, Clone)]
pub struct Member {
    /// Its name, or how the user named it when that is all there is (an IP).
    pub name: String,
    /// Its node id, when known before it answers.
    pub id: Option<String>,
    /// Where it will be reached, `ip:port`, when known before it answers.
    pub address: Option<String>,
    pub target: Target,
}

impl Member {
    /// The same device named twice is one device.
    fn key(&self) -> String {
        self.id
            .clone()
            .or_else(|| self.address.clone())
            .unwrap_or_else(|| self.name.clone())
    }
}

/// The devices picked, and what the scan found, for the caller to note
/// (`Nodes::note_found`) as `nodes list` does.
#[derive(Debug, Default)]
pub struct Selection {
    pub members: Vec<Member>,
    pub found: Vec<Found>,
}

/// What one device made of the run.
#[derive(Debug)]
pub struct Outcome<T> {
    pub member: Member,
    pub result: Result<T, String>,
}

/// The names in a `--node` value: comma-separated, trimmed, each once.
pub fn names(node: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in node
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        if !names.iter().any(|known| known == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// The devices `names` and `wanted` tags stand for: every named device, and
/// every device, known or found by the scan, that has all of the tags
/// (`tags::matches`, with `unclaimed` from the scan). `scan` is called at
/// most once. A name that stands for no device, or a selection with no
/// device at all, is an error before anything runs.
pub fn select(
    names: &[String],
    wanted: &[String],
    nodes: &Nodes,
    scan: impl FnOnce() -> Vec<Found>,
) -> Result<Selection, String> {
    let targets = names
        .iter()
        .map(|name| Ok((name.clone(), connect::resolve(Some(name), nodes)?)))
        .collect::<Result<Vec<_>, String>>()?;
    let needs_scan = !wanted.is_empty()
        || targets
            .iter()
            .any(|(_, target)| matches!(target, Target::Named { known: None, .. }));
    let found = if needs_scan { scan() } else { Vec::new() };

    let mut members = Vec::new();
    for (typed, target) in targets {
        members.push(named(&typed, target, nodes, &found)?);
    }
    if !wanted.is_empty() {
        for node in &nodes.nodes {
            let seen = found
                .iter()
                .find(|found| found.id.as_deref() == Some(node.id.as_str()));
            let has = match seen {
                Some(seen) => tags::effective(&seen.tags, seen.claimed),
                None => node.tags.clone(),
            };
            if tags::matches(&has, wanted) {
                members.push(match seen {
                    Some(seen) => reached(seen, Some(node), None),
                    None => known(node, None),
                });
            }
        }
        for seen in &found {
            let is_known = seen
                .id
                .as_deref()
                .is_some_and(|id| nodes.by_id(id).is_some());
            if !is_known && tags::matches(&tags::effective(&seen.tags, seen.claimed), wanted) {
                members.push(reached(seen, None, None));
            }
        }
    }

    let mut picked: Vec<Member> = Vec::new();
    for member in members {
        if !picked.iter().any(|seen| seen.key() == member.key()) {
            picked.push(member);
        }
    }
    if picked.is_empty() {
        return Err(match wanted {
            [] => "no device named".to_string(),
            [tag] => format!("no device has the tag {tag}"),
            tags => format!("no device has every one of the tags {}", tags.join(", ")),
        });
    }
    Ok(Selection {
        members: picked,
        found,
    })
}

/// The device a name the user typed stands for.
fn named(typed: &str, target: Target, nodes: &Nodes, found: &[Found]) -> Result<Member, String> {
    match target {
        Target::Local(_) => Ok(Member {
            name: "local".to_string(),
            id: None,
            address: None,
            target,
        }),
        Target::Remote {
            address,
            ref expected,
            ..
        } => Ok(Member {
            name: typed.to_string(),
            id: expected.clone(),
            address: Some(address.to_string()),
            target,
        }),
        Target::Named {
            known: Some(node),
            port,
            ..
        } => Ok(match seen_as(&node, found) {
            Some(seen) => reached(seen, Some(&node), port),
            None => known(&node, port),
        }),
        Target::Named {
            name,
            port,
            known: None,
            start,
        } => {
            let (name, node) = if start {
                connect::pick(&name, nodes, found)?
            } else {
                (name, None)
            };
            let seen = found
                .iter()
                .find(|found| found.name == name)
                .or_else(|| node.as_ref().and_then(|node| seen_as(node, found)));
            match (seen, node) {
                (Some(seen), node) => Ok(reached(seen, node.as_ref(), port)),
                (None, Some(node)) => Ok(known(&node, port)),
                (None, None) => Err(format!(
                    "{typed}: not found on the network (mDNS) and not a known node"
                )),
            }
        }
    }
}

/// A known device as the scan found it, if it did.
fn seen_as<'a>(node: &Node, found: &'a [Found]) -> Option<&'a Found> {
    found
        .iter()
        .find(|found| found.id.as_deref() == Some(node.id.as_str()))
}

/// A device the scan found, reached where it announced itself; held to its
/// pin when it is known.
fn reached(seen: &Found, node: Option<&Node>, port: Option<u16>) -> Member {
    let address = SocketAddr::new(seen.address.ip(), port.unwrap_or(seen.address.port()));
    let id = node.map(|node| node.id.clone()).or_else(|| seen.id.clone());
    Member {
        name: seen.name.clone(),
        id: id.clone(),
        address: Some(address.to_string()),
        target: Target::Remote {
            address,
            expected: id,
            label: seen.name.clone(),
        },
    }
}

/// A device a client already has as a node (the GUI's marked rows): its
/// cached address first, as `--node NAME` does.
pub fn member(node: &Node) -> Member {
    known(node, None)
}

/// A known device the scan did not find (or no scan was made): its cached
/// address first, as `--node NAME` does.
fn known(node: &Node, port: Option<u16>) -> Member {
    Member {
        name: node.name.clone(),
        id: Some(node.id.clone()),
        address: Some(node.address.clone()),
        target: Target::Named {
            name: node.name.clone(),
            port,
            known: Some(node.clone()),
            start: false,
        },
    }
}

/// `run` on every member, at most `limit` at once, each on a thread of its
/// own. The outcomes come back in the members' order.
pub fn each<T: Send>(
    members: Vec<Member>,
    limit: usize,
    run: impl Fn(&Member) -> Result<T, String> + Sync,
) -> Vec<Outcome<T>> {
    let next = AtomicUsize::new(0);
    let results: Vec<Mutex<Option<Result<T, String>>>> =
        members.iter().map(|_| Mutex::new(None)).collect();
    let threads = limit.clamp(1, members.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let at = next.fetch_add(1, Ordering::Relaxed);
                let Some(member) = members.get(at) else {
                    break;
                };
                let result = run(member);
                *results[at].lock().unwrap_or_else(|err| err.into_inner()) = Some(result);
            });
        }
    });
    members
        .into_iter()
        .zip(results)
        .map(|(member, result)| Outcome {
            member,
            result: result
                .into_inner()
                .unwrap_or_else(|err| err.into_inner())
                .unwrap_or_else(|| Err("did not run".to_string())),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    fn node(name: &str, id: &str, tags: &[&str]) -> Node {
        Node {
            id: id.to_string(),
            name: name.to_string(),
            address: format!("10.0.0.{}:7400", id.len()),
            fingerprint: "ff".to_string(),
            token: Some("t".to_string()),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
        }
    }

    fn found(name: &str, id: &str, ip: u8, claimed: bool, tags: &[&str]) -> Found {
        Found {
            name: name.to_string(),
            address: SocketAddr::from(([10, 0, 1, ip], 7400)),
            id: Some(id.to_string()),
            fingerprint: None,
            claimed: Some(claimed),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
        }
    }

    fn known_nodes() -> Nodes {
        Nodes {
            nodes: vec![
                node("lobby", "aaaa", &["lobby", "floor-2"]),
                node("kitchen", "bbbb", &["floor-2"]),
            ],
        }
    }

    fn picked(selection: &Selection) -> Vec<&str> {
        selection
            .members
            .iter()
            .map(|member| member.name.as_str())
            .collect()
    }

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn a_node_value_is_split_on_commas_each_name_once() {
        assert_eq!(names(" a, b,,a ,c"), list(&["a", "b", "c"]));
        assert!(names(" , ").is_empty());
    }

    #[test]
    fn known_names_need_no_scan() {
        let nodes = known_nodes();
        let selection = select(&list(&["lobby", "kitchen"]), &[], &nodes, || {
            panic!("no scan for known names")
        })
        .unwrap();
        assert_eq!(picked(&selection), ["lobby", "kitchen"]);
        assert!(matches!(
            selection.members[0].target,
            Target::Named { start: false, .. }
        ));
    }

    #[test]
    fn a_device_named_twice_runs_once() {
        let nodes = known_nodes();
        let selection = select(&list(&["lobby", "10.0.0.4:7400"]), &[], &nodes, Vec::new).unwrap();
        // 10.0.0.4:7400 is lobby's cached address, so it is lobby.
        assert_eq!(picked(&selection), ["lobby"]);
    }

    #[test]
    fn every_tag_must_match_and_the_scan_is_made_once() {
        let nodes = known_nodes();
        let scans = AtomicUsize::new(0);
        let selection = select(&[], &list(&["floor-2", "lobby"]), &nodes, || {
            scans.fetch_add(1, Ordering::Relaxed);
            Vec::new()
        })
        .unwrap();
        assert_eq!(picked(&selection), ["lobby"]);
        assert_eq!(scans.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn unclaimed_devices_come_from_the_scan() {
        let nodes = known_nodes();
        let fresh = || {
            vec![
                found("tessaro-1", "cccc", 7, false, &[]),
                found("tessaro-2", "dddd", 8, false, &[]),
                found("lobby", "aaaa", 9, true, &["lobby"]),
            ]
        };
        let selection = select(&[], &list(&["unclaimed"]), &nodes, fresh).unwrap();
        assert_eq!(picked(&selection), ["tessaro-1", "tessaro-2"]);
        match &selection.members[0].target {
            Target::Remote {
                address, expected, ..
            } => {
                assert_eq!(address.to_string(), "10.0.1.7:7400");
                assert_eq!(expected.as_deref(), Some("cccc"));
            }
            other => panic!("not reached where it announced itself: {other:?}"),
        }
        assert_eq!(selection.found.len(), 3);
    }

    #[test]
    fn a_known_device_found_is_reached_where_it_announced_and_tagged_as_it_says() {
        let nodes = known_nodes();
        // The scan says kitchen is in the lobby now; the stale cache says not.
        let selection = select(&[], &list(&["lobby"]), &nodes, || {
            vec![
                found("kitchen", "bbbb", 5, true, &["lobby"]),
                found("lobby", "aaaa", 4, true, &[]),
            ]
        })
        .unwrap();
        assert_eq!(picked(&selection), ["kitchen"]);
        assert_eq!(
            selection.members[0].address.as_deref(),
            Some("10.0.1.5:7400")
        );
    }

    #[test]
    fn a_start_of_a_name_picks_one_device() {
        let nodes = known_nodes();
        let selection = select(&list(&["kit"]), &[], &nodes, Vec::new).unwrap();
        assert_eq!(picked(&selection), ["kitchen"]);
        assert!(select(&list(&["nowhere"]), &[], &nodes, Vec::new).is_err());
    }

    #[test]
    fn nothing_selected_is_an_error_naming_the_tags() {
        let nodes = known_nodes();
        let err = select(&[], &list(&["basement"]), &nodes, Vec::new).unwrap_err();
        assert_eq!(err, "no device has the tag basement");
    }

    #[test]
    fn each_keeps_the_order_and_the_limit() {
        let nodes = Nodes {
            nodes: (0..6)
                .map(|at| node(&format!("n{at}"), &format!("id{at}"), &[]))
                .collect(),
        };
        let all: Vec<String> = (0..6).map(|at| format!("n{at}")).collect();
        let members = select(&all, &[], &nodes, Vec::new).unwrap().members;
        let running = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        let failed = AtomicBool::new(false);
        let outcomes = each(members, 2, |member| {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            running.fetch_sub(1, Ordering::SeqCst);
            if member.name == "n3" {
                failed.store(true, Ordering::SeqCst);
                return Err("broken".to_string());
            }
            Ok(member.name.clone())
        });
        assert!(most.load(Ordering::SeqCst) <= 2);
        assert!(failed.load(Ordering::SeqCst));
        let names: Vec<&str> = outcomes.iter().map(|o| o.member.name.as_str()).collect();
        assert_eq!(names, ["n0", "n1", "n2", "n3", "n4", "n5"]);
        assert_eq!(outcomes[3].result, Err("broken".to_string()));
        assert_eq!(outcomes[5].result.as_deref(), Ok("n5"));
    }
}
