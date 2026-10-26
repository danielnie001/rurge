//! What undoing a change that outlives rurge takes (M4 design 9.2), and the
//! backup `state.json` keeps of it.

use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// What undoing a change that outlives rurge takes (M4 design 9.2). Typed,
/// never a command line: an edited state.json can make rurge do only what
/// it would have done anyway.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Undo {
    /// macOS: a network service's DNS servers before the takeover (empty: none were set).
    MacDns {
        service: String,
        servers: Vec<IpAddr>,
    },
    /// Linux: `net.ipv4.conf.all.rp_filter` before it was loosened.
    RpFilter { value: u8 },
    /// The file backend: one of its steps.
    Test { step: String },
}

/// When a close undoes it: with the DNS (first) or after the device (last).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Dns,
    Adjust,
}

impl Undo {
    /// Whether a close undoes it with the DNS, before the traffic goes back to
    /// the physical adapter, or after the device is gone (P12).
    pub fn stage(&self) -> Stage {
        match self {
            Undo::MacDns { .. } => Stage::Dns,
            Undo::Test { step } if step == "take_dns" => Stage::Dns,
            Undo::RpFilter { .. } | Undo::Test { .. } => Stage::Adjust,
        }
    }

    /// The name this undo goes by in the log, in the file backend's actions and
    /// in its `fail_undo`.
    pub(super) fn label(&self) -> String {
        match self {
            Undo::Test { step } => step.clone(),
            Undo::MacDns { service, .. } => format!("mac_dns {service}"),
            Undo::RpFilter { value } => format!("rp_filter {value}"),
        }
    }
}

/// `state.json`'s `enhanced_backup`: `{"undo": [...]}` in the order the changes were made.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backup {
    pub undo: Vec<Undo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `take_dns` goes back with the DNS, a `rp_filter` and a `Test` step
    /// that is not the takeover of it after the device.
    #[test]
    fn the_stage_of_each_undo_is_the_order_a_close_goes_in() {
        let service = |servers| Undo::MacDns {
            service: "Wi-Fi".to_string(),
            servers,
        };
        assert_eq!(service(vec![]).stage(), Stage::Dns);
        assert_eq!(
            service(vec!["1.1.1.1".parse().unwrap()]).stage(),
            Stage::Dns
        );
        assert_eq!(Undo::RpFilter { value: 1 }.stage(), Stage::Adjust);
        assert_eq!(
            Undo::Test {
                step: "take_dns".to_string()
            }
            .stage(),
            Stage::Dns
        );
        assert_eq!(
            Undo::Test {
                step: "adjust".to_string()
            }
            .stage(),
            Stage::Adjust
        );
    }

    /// The backup is the shape `state.json` keeps, and an undo of a kind this
    /// version does not know does not read as one it does.
    #[test]
    fn the_backup_round_trips_and_an_unknown_kind_does_not() {
        let backup = Backup {
            undo: vec![
                Undo::Test {
                    step: "take_dns".to_string(),
                },
                Undo::RpFilter { value: 0 },
                Undo::MacDns {
                    service: "Wi-Fi".to_string(),
                    servers: vec!["9.9.9.9".parse().unwrap()],
                },
            ],
        };
        let text = serde_json::to_string(&backup).unwrap();
        assert!(text.contains(r#""kind":"rp_filter""#), "{text}");
        assert_eq!(serde_json::from_str::<Backup>(&text).unwrap(), backup);
        assert!(serde_json::from_str::<Backup>(r#"{"undo":[{"kind":"dns","x":1}]}"#).is_err());
        assert!(serde_json::from_str::<Backup>(r#"{"x":1}"#).is_err());
        assert_eq!(
            serde_json::from_str::<Backup>(r#"{"undo":[]}"#).unwrap(),
            Backup::default()
        );
    }

    #[test]
    fn each_undo_has_a_name_the_file_backend_and_the_log_can_use() {
        assert_eq!(
            Undo::Test {
                step: "take_dns".to_string()
            }
            .label(),
            "take_dns"
        );
        assert_eq!(Undo::RpFilter { value: 1 }.label(), "rp_filter 1");
        assert_eq!(
            Undo::MacDns {
                service: "Wi-Fi".to_string(),
                servers: vec![]
            }
            .label(),
            "mac_dns Wi-Fi"
        );
    }
}
