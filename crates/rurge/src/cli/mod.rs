pub mod api_client;
pub mod check;
pub mod control;
pub mod dns;
pub mod network;
pub mod rule;
pub mod run;
pub mod runtime;
pub mod service;
pub mod sysproxy;

use rurge_config::config::Platform;
use rurge_config::requirement::Environment;
use rurge_platform::sysinfo::{self, SystemInfo};

/// How a switch reads its environment variable: the usual spellings of yes
/// and no (`1` / `0`, `true` / `false`, `yes` / `no`, `on` / `off`, `y` /
/// `n`, `t` / `f`), anything else an error — clap's own reading of a `bool`
/// takes `true` and `false` alone.
pub fn switch() -> clap::builder::BoolishValueParser {
    clap::builder::BoolishValueParser::new()
}

pub fn environment(platform: Platform, core_version: u64) -> Environment {
    environment_from(platform, core_version, sysinfo::read(), &|name| {
        std::env::var(name).ok()
    })
}

/// What a profile's requirement expressions see. The version, the model, the
/// language and the name are the operating system's own word; what it does
/// not say is what has always been reported: `unknown`, the CPU architecture,
/// the language `$LANG` names (`en-US` when it names none) and the computer
/// name from `$COMPUTERNAME` or `$HOSTNAME` (`rurge` without them). `$LANG` is
/// asked on its own: a system that found no language because `LC_ALL` says `C`
/// still gets the language `$LANG` names.
///
/// `--platform` has a profile read as if on another system, on this machine.
/// This machine's version is not that system's, and a version test would
/// mean nothing under the other name, so the version is reported only for the
/// platform the machine runs. The model, the language and the name are this
/// machine's whichever platform the profile is read for.
fn environment_from(
    platform: Platform,
    core_version: u64,
    info: SystemInfo,
    var: &dyn Fn(&str) -> Option<String>,
) -> Environment {
    let system_version = info
        .version
        .filter(|_| platform == Platform::current())
        .unwrap_or_else(|| "unknown".to_string());
    let language = info.language.unwrap_or_else(|| {
        var("LANG")
            .and_then(|lang| sysinfo::language_of(&lang))
            .unwrap_or_else(|| "en-US".to_string())
    });
    let device_name = info.name.unwrap_or_else(|| {
        var("COMPUTERNAME")
            .or_else(|| var("HOSTNAME"))
            .unwrap_or_else(|| "rurge".to_string())
    });
    Environment {
        core_version,
        system: platform.system_name().to_string(),
        system_version,
        device_model: info
            .model
            .unwrap_or_else(|| std::env::consts::ARCH.to_string()),
        language,
        device_name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lookup over the variables given.
    fn vars<'a>(set: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            set.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, value)| value.to_string())
        }
    }

    fn the_machine() -> SystemInfo {
        SystemInfo {
            version: Some("System Version 14.6.1 (Build 23G93)".to_string()),
            model: Some("Mac15,6".to_string()),
            language: Some("fr-FR".to_string()),
            name: Some("Tim's MacBook Pro".to_string()),
        }
    }

    /// What `environment` reported before the system was asked, for a system
    /// that says nothing: the same words, whatever the variables hold — except
    /// the language of three `$LANG` values, which is now read the way the
    /// system's own locale variables are: `POSIX` and `@euro` name no language
    /// (they were reported as themselves), and `sr_RS@latin` is `sr-RS` (it was
    /// `sr-RS@latin`).
    #[test]
    fn a_system_that_says_nothing_leaves_what_the_variables_always_gave() {
        let host = Platform::current();
        let none =
            |set: &[(&str, &str)]| environment_from(host, 22, SystemInfo::default(), &vars(set));
        assert_eq!(
            none(&[]),
            Environment {
                core_version: 22,
                system: host.system_name().to_string(),
                system_version: "unknown".to_string(),
                device_model: std::env::consts::ARCH.to_string(),
                language: "en-US".to_string(),
                device_name: "rurge".to_string(),
            }
        );
        for (lang, language) in [
            ("zh_CN.UTF-8", "zh-CN"),
            ("en_US", "en-US"),
            ("C", "en-US"),
            ("C.UTF-8", "en-US"),
            ("", "en-US"),
            ("sr_RS.UTF-8@latin", "sr-RS"),
            (".UTF-8", "en-US"),
            // the three that are read differently now
            ("POSIX", "en-US"),
            ("sr_RS@latin", "sr-RS"),
            ("@euro", "en-US"),
        ] {
            assert_eq!(none(&[("LANG", lang)]).language, language, "{lang:?}");
        }
        // `$LANG` is asked on its own: `LC_ALL` is read only by the system's
        // own reader, which has already said it found no language
        assert_eq!(
            none(&[("LC_ALL", "C"), ("LANG", "de_DE.UTF-8")]).language,
            "de-DE"
        );
        assert_eq!(none(&[("COMPUTERNAME", "BOX")]).device_name, "BOX");
        assert_eq!(none(&[("HOSTNAME", "box")]).device_name, "box");
        assert_eq!(
            none(&[("COMPUTERNAME", "BOX"), ("HOSTNAME", "box")]).device_name,
            "BOX"
        );
    }

    #[test]
    fn what_the_system_says_replaces_only_its_own_value() {
        let host = Platform::current();
        let base = environment_from(host, 22, SystemInfo::default(), &vars(&[]));
        let with = |info: SystemInfo| environment_from(host, 22, info, &vars(&[]));
        let machine = the_machine();
        assert_eq!(
            with(SystemInfo {
                version: machine.version.clone(),
                ..SystemInfo::default()
            }),
            Environment {
                system_version: "System Version 14.6.1 (Build 23G93)".to_string(),
                ..base.clone()
            }
        );
        assert_eq!(
            with(SystemInfo {
                model: machine.model.clone(),
                ..SystemInfo::default()
            }),
            Environment {
                device_model: "Mac15,6".to_string(),
                ..base.clone()
            }
        );
        assert_eq!(
            with(SystemInfo {
                language: machine.language.clone(),
                ..SystemInfo::default()
            }),
            Environment {
                language: "fr-FR".to_string(),
                ..base.clone()
            }
        );
        assert_eq!(
            with(SystemInfo {
                name: machine.name.clone(),
                ..SystemInfo::default()
            }),
            Environment {
                device_name: "Tim's MacBook Pro".to_string(),
                ..base
            }
        );
        // the system's word comes before the variables'
        let env = environment_from(
            host,
            22,
            machine,
            &vars(&[("LANG", "de_DE.UTF-8"), ("COMPUTERNAME", "BOX")]),
        );
        assert_eq!(
            (env.language.as_str(), env.device_name.as_str()),
            ("fr-FR", "Tim's MacBook Pro")
        );
    }

    /// `--platform` evaluates a profile as if on another system, on this
    /// machine: the version is that system's to give, and this machine's is
    /// not; the model, the language and the name stay this machine's.
    #[test]
    fn the_version_is_reported_only_for_the_platform_the_machine_runs() {
        let host = Platform::current();
        let env = environment_from(host, 22, the_machine(), &vars(&[]));
        assert_eq!(env.system_version, "System Version 14.6.1 (Build 23G93)");
        let others = [Platform::Windows, Platform::Linux, Platform::MacOs]
            .into_iter()
            .filter(|p| *p != host)
            .collect::<Vec<_>>();
        assert_eq!(others.len(), 2);
        for other in others {
            let env = environment_from(other, 22, the_machine(), &vars(&[]));
            assert_eq!(env.system, other.system_name());
            assert_eq!(env.system_version, "unknown", "{other:?}");
            assert_eq!(
                (
                    env.device_model.as_str(),
                    env.language.as_str(),
                    env.device_name.as_str()
                ),
                ("Mac15,6", "fr-FR", "Tim's MacBook Pro"),
                "{other:?}"
            );
        }
    }
}
