//! What the operating system says about the machine: the four values a
//! profile's requirement expressions can read (`SYSTEM_VERSION`,
//! `DEVICE_MODEL`, `LANGUAGE`, `DEVICE_NAME`).
//!
//! Best effort and silent: whatever the system does not say, or says in a way
//! that is not understood, is `None`; nothing fails and nothing is logged. As
//! in `dirs` and `sysproxy`, the rules of every platform are compiled and
//! unit-tested on every host: the macOS and Linux readers take their commands,
//! files and environment as [`Sources`], and only the registry reader and the
//! choice of the reader for this host are `cfg`-gated.

use crate::command::CommandRunner;
use crate::dirs::EnvLookup;
use std::sync::OnceLock;

/// What the system says; a value it does not say is `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemInfo {
    /// `System Version 14.6.1 (Build 23G93)`, the way the manual shows it.
    pub version: Option<String>,
    /// The hardware model: `Mac15,6`, `Latitude 7440`.
    pub model: Option<String>,
    /// A language tag: `en-US`.
    pub language: Option<String>,
    /// The name the machine was given.
    pub name: Option<String>,
}

/// Reads this system. Best effort: whatever the system does not say is
/// `None`, and nothing fails. The system is read once per process — macOS runs
/// programs to be told —, so a later call returns what the first one found.
pub fn read() -> SystemInfo {
    static INFO: OnceLock<SystemInfo> = OnceLock::new();
    INFO.get_or_init(read_here).clone()
}

#[cfg(windows)]
fn read_here() -> SystemInfo {
    read_windows()
}

/// A Unix that is not macOS is read as Linux is, the way `dirs` and `sysproxy`
/// treat it.
#[cfg(unix)]
fn read_here() -> SystemInfo {
    let sources = Sources {
        runner: &crate::command::SystemRunner,
        file: &read_file,
        env: &|var| std::env::var_os(var),
    };
    if cfg!(target_os = "macos") {
        read_macos(&sources)
    } else {
        read_linux(&sources)
    }
}

#[cfg(not(any(windows, unix)))]
fn read_here() -> SystemInfo {
    SystemInfo::default()
}

/// Where a reader gets what it reads; [`read`] passes the real ones.
pub struct Sources<'a> {
    pub runner: &'a dyn CommandRunner,
    /// A file's text; `None` when it cannot be read.
    pub file: &'a dyn Fn(&str) -> Option<String>,
    pub env: EnvLookup<'a>,
}

/// macOS: the system version from the property list the system keeps it in,
/// the rest from `sysctl`, `defaults` and `scutil`.
pub fn read_macos(from: &Sources<'_>) -> SystemInfo {
    let ask = |cmd: &[&str]| {
        let cmd: Vec<String> = cmd.iter().map(|part| part.to_string()).collect();
        from.runner.run(&cmd).ok()
    };
    SystemInfo {
        version: (from.file)("/System/Library/CoreServices/SystemVersion.plist")
            .and_then(|plist| macos_version(&plist))
            .map(|(version, build)| system_version(&version, &build)),
        model: ask(&["sysctl", "-n", "hw.model"]).and_then(|out| model(&out)),
        language: ask(&["defaults", "read", "-g", "AppleLanguages"])
            .and_then(|out| apple_languages(&out))
            .or_else(|| language_from_variables(from)),
        // the name in System Settings; a Mac that has none is still known by
        // its Bonjour name
        name: ask(&["scutil", "--get", "ComputerName"])
            .and_then(|out| name(&out))
            .or_else(|| ask(&["scutil", "--get", "LocalHostName"]).and_then(|out| name(&out))),
    }
}

/// Linux: files under `/proc` and `/sys`, and the locale variables. No
/// program is run.
pub fn read_linux(from: &Sources<'_>) -> SystemInfo {
    let first_of = |paths: &[&str], clean: fn(&str) -> Option<String>| {
        paths
            .iter()
            .find_map(|path| (from.file)(path).and_then(|text| clean(&text)))
    };
    SystemInfo {
        version: (from.file)("/proc/sys/kernel/osrelease")
            .and_then(|release| linux_version(&release))
            .map(|(version, build)| system_version(&version, &build)),
        // the firmware's product name; boards that have no firmware of that
        // kind (a Raspberry Pi) describe themselves in the device tree
        model: first_of(
            &["/sys/class/dmi/id/product_name", "/proc/device-tree/model"],
            model,
        ),
        language: language_from_variables(from),
        name: first_of(&["/proc/sys/kernel/hostname", "/etc/hostname"], name)
            .or_else(|| variable(from, "HOSTNAME").and_then(|host| name(&host))),
    }
}

/// Windows: the registry. A value read from a key that is not there, or that
/// is not of the type it should be, is not there.
#[cfg(windows)]
fn read_windows() -> SystemInfo {
    use windows_registry::{CURRENT_USER, LOCAL_MACHINE};

    let version = LOCAL_MACHINE
        .open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .ok()
        .and_then(|key| {
            let major = key.get_u32("CurrentMajorVersionNumber").ok()?;
            let minor = key.get_u32("CurrentMinorVersionNumber").ok()?;
            let build = name(&key.get_string("CurrentBuildNumber").ok()?)?;
            let (version, build) = windows_version(major, minor, &build, key.get_u32("UBR").ok());
            Some(system_version(&version, &build))
        });
    SystemInfo {
        version,
        model: registry_string(
            LOCAL_MACHINE,
            r"HARDWARE\DESCRIPTION\System\BIOS",
            "SystemProductName",
        )
        .and_then(|raw| model(&raw)),
        // the display language when the user chose one; otherwise the first
        // of the languages the user lists; otherwise the regional format
        language: registry_first_string(
            CURRENT_USER,
            r"Control Panel\Desktop",
            "PreferredUILanguages",
        )
        .or_else(|| {
            registry_first_string(
                CURRENT_USER,
                r"Control Panel\International\User Profile",
                "Languages",
            )
        })
        .or_else(|| registry_string(CURRENT_USER, r"Control Panel\International", "LocaleName")),
        // the DNS host name, in the case the user typed it (`COMPUTERNAME` is
        // the NetBIOS name, which is upper-cased)
        name: registry_string(
            LOCAL_MACHINE,
            r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters",
            "Hostname",
        ),
    }
}

/// A string value under `hive`; `None` when the key or the value is not there,
/// or the value is not a string.
#[cfg(windows)]
fn registry_string(hive: &windows_registry::Key, path: &str, value: &str) -> Option<String> {
    name(&hive.open(path).ok()?.get_string(value).ok()?)
}

/// The first of a list under `hive` (`REG_MULTI_SZ`; the first is the
/// preferred one). The crate ends the list with empty strings, which `name`
/// does not take. A string is a list of one.
#[cfg(windows)]
fn registry_first_string(hive: &windows_registry::Key, path: &str, value: &str) -> Option<String> {
    let list = hive.open(path).ok()?.get_multi_string(value).ok()?;
    list.iter().find_map(|item| name(item))
}

/// A file's text, or `None` when there is no such file, it cannot be opened or
/// it is not text.
#[cfg(any(unix, test))]
fn read_file(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// The language the locale variables name.
fn language_from_variables(from: &Sources<'_>) -> Option<String> {
    let lc_all = variable(from, "LC_ALL");
    let lc_messages = variable(from, "LC_MESSAGES");
    let lang = variable(from, "LANG");
    posix_language(lc_all.as_deref(), lc_messages.as_deref(), lang.as_deref())
}

/// An environment variable that holds text.
fn variable(from: &Sources<'_>, var: &str) -> Option<String> {
    (from.env)(var).and_then(|value| value.into_string().ok())
}

/// `System Version 14.6.1 (Build 23G93)`, the shape of the manual's example
/// (`System Version 17.4.1 (Build 21E236)`). macOS keeps the two parts apart;
/// Windows and Linux are made to fit the shape.
fn system_version(version: &str, build: &str) -> String {
    format!("System Version {version} (Build {build})")
}

/// `ProductVersion` and `ProductBuildVersion` of `SystemVersion.plist`, in
/// either order; `None` when either is missing or the text is not a plist.
fn macos_version(plist: &str) -> Option<(String, String)> {
    Some((
        plist_string(plist, "ProductVersion")?,
        plist_string(plist, "ProductBuildVersion")?,
    ))
}

/// The text of the `<string>` that follows `<key>key</key>`.
fn plist_string(plist: &str, key: &str) -> Option<String> {
    let marker = format!("<key>{key}</key>");
    let after = &plist[plist.find(&marker)? + marker.len()..];
    let text = after.trim_start().strip_prefix("<string>")?;
    name(&text[..text.find("</string>")?])
}

/// `10.0.22631` and `22631.4317`: the version, and the build with the update
/// revision (`UBR`) after it when the registry has one.
#[cfg(any(windows, test))]
fn windows_version(major: u32, minor: u32, build: &str, ubr: Option<u32>) -> (String, String) {
    let version = format!("{major}.{minor}.{build}");
    match ubr {
        Some(revision) => (version, format!("{build}.{revision}")),
        None => (version, build.to_string()),
    }
}

/// The kernel release `6.8.0-45-generic`: the version is the numbers it
/// begins with (`6.8.0`), the build is all of it.
fn linux_version(osrelease: &str) -> Option<(String, String)> {
    let build = osrelease.trim();
    let digits = |from: usize| {
        build.as_bytes()[from..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
    };
    let mut end = digits(0);
    if end == 0 {
        return None;
    }
    // `.` and more digits, as often as there are
    while build.as_bytes().get(end) == Some(&b'.') {
        let more = digits(end + 1);
        if more == 0 {
            break;
        }
        end += 1 + more;
    }
    Some((build[..end].to_string(), build.to_string()))
}

/// A model as the firmware gives it, unless it is what a maker leaves in the
/// field when it did not fill it in: a template that begins `To be filled`,
/// `OEM` or `System Product` (in any case), or one of the words that stand for
/// "no value" (`None`, `Default string`, `N/A`, `Unknown`, …) as the whole text.
fn model(raw: &str) -> Option<String> {
    // a template goes on after its first words (`To Be Filled By O.E.M.`)
    const TEMPLATES: [&str; 6] = [
        "To be filled",
        "To be set",
        "OEM",
        "O.E.M.",
        "System Product",
        "Unknown Product",
    ];
    // a word that stands for "no value" is the whole text
    const NO_VALUE: [&str; 13] = [
        "None",
        "Default string",
        "Undefined",
        "Not Specified",
        "Not Applicable",
        "Not Defined",
        "Not Available",
        "INVALID",
        "Type1ProductConfigId",
        "TBD by OEM",
        "N/A",
        "Unknown",
        "System Name",
    ];
    // bytes, not characters: where a prefix ends may be inside a character
    let begins_with = |text: &str, prefix: &str| {
        text.as_bytes()
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
    };
    name(raw).filter(|model| {
        !TEMPLATES
            .iter()
            .any(|template| begins_with(model, template))
            && !NO_VALUE.iter().any(|word| word.eq_ignore_ascii_case(model))
    })
}

/// The language a locale name stands for: `zh_CN.UTF-8` is `zh-CN` — what
/// comes before the encoding and the modifier (`sr_RS.UTF-8@latin` is
/// `sr-RS`). `C` and `POSIX` name no language.
pub fn language_of(locale: &str) -> Option<String> {
    let locale = locale.trim();
    let language = &locale[..locale.find(['.', '@']).unwrap_or(locale.len())];
    if matches!(language, "" | "C" | "POSIX") {
        return None;
    }
    Some(language.replace('_', "-"))
}

/// The language of the locale the first of the three variables names — the
/// first that is set to something, as POSIX has it. A variable that names no
/// language is not skipped in favour of the next: `LC_ALL=C` is the locale,
/// whatever `LANG` says.
fn posix_language(
    lc_all: Option<&str>,
    lc_messages: Option<&str>,
    lang: Option<&str>,
) -> Option<String> {
    [lc_all, lc_messages, lang]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .and_then(language_of)
}

/// The first language of `defaults read -g AppleLanguages`, a list that is
/// printed one language to a line, in quotes — or, by older systems, on one
/// line and in quotes only where the tag has a hyphen: `(en, "zh-Hans", ja)`.
fn apple_languages(output: &str) -> Option<String> {
    let list = output.trim().strip_prefix('(')?;
    let first = list.split([',', ')']).next()?;
    name(first.trim().trim_matches('"'))
}

/// A name or a model as a file or a program gives it: without the white space
/// (and the NUL of the device tree) around it. Nothing is no name.
fn name(raw: &str) -> Option<String> {
    let name = raw.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::FakeRunner;
    use std::ffi::OsString;
    use std::io;

    /// `SystemVersion.plist` as macOS writes it: XML, a tab per level.
    const PLIST: &str = concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
        "<plist version=\"1.0\">\n",
        "<dict>\n",
        "\t<key>ProductBuildVersion</key>\n",
        "\t<string>23G93</string>\n",
        "\t<key>ProductCopyright</key>\n",
        "\t<string>1983-2024 Apple Inc.</string>\n",
        "\t<key>ProductName</key>\n",
        "\t<string>macOS</string>\n",
        "\t<key>ProductUserVisibleVersion</key>\n",
        "\t<string>14.6.1</string>\n",
        "\t<key>ProductVersion</key>\n",
        "\t<string>14.6.1</string>\n",
        "\t<key>iOSSupportVersion</key>\n",
        "\t<string>17.6</string>\n",
        "</dict>\n",
        "</plist>\n",
    );

    fn pair(a: &str, b: &str) -> Option<(String, String)> {
        Some((a.to_string(), b.to_string()))
    }

    fn some(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn a_version_reads_the_way_the_manual_shows_it() {
        assert_eq!(
            system_version("14.6.1", "23G93"),
            "System Version 14.6.1 (Build 23G93)"
        );
        assert_eq!(
            system_version("6.8.0", "6.8.0-45-generic"),
            "System Version 6.8.0 (Build 6.8.0-45-generic)"
        );
    }

    #[test]
    fn the_macos_version_is_the_product_version_and_the_build_version() {
        assert_eq!(macos_version(PLIST), pair("14.6.1", "23G93"));
        // the keys in the other order, with no white space between the tags
        assert_eq!(
            macos_version(
                "<plist><dict><key>ProductVersion</key><string>13.0</string>\
                 <key>ProductBuildVersion</key><string>22A380</string></dict></plist>"
            ),
            pair("13.0", "22A380")
        );
        // the keys are matched whole: `ProductUserVisibleVersion` is not `ProductVersion`
        assert_eq!(
            macos_version(
                "<dict><key>ProductUserVisibleVersion</key><string>14.6.1</string>\
                 <key>ProductBuildVersion</key><string>23G93</string></dict>"
            ),
            None
        );
        // a key that is missing, or has no text, or whose value is not a string
        assert_eq!(
            macos_version("<dict><key>ProductVersion</key><string>14.6.1</string></dict>"),
            None
        );
        assert_eq!(
            macos_version("<dict><key>ProductBuildVersion</key><string>23G93</string></dict>"),
            None
        );
        assert_eq!(
            macos_version(
                "<dict><key>ProductVersion</key><string> </string>\
                 <key>ProductBuildVersion</key><string>23G93</string></dict>"
            ),
            None
        );
        assert_eq!(
            macos_version(
                "<dict><key>ProductVersion</key><integer>14</integer>\
                 <key>ProductBuildVersion</key><string>23G93</string></dict>"
            ),
            None
        );
        // not a property list
        for text in ["", "ProductVersion 14.6.1", "<html></html>", "\u{1}\u{2}"] {
            assert_eq!(macos_version(text), None, "{text:?}");
        }
    }

    #[test]
    fn the_windows_version_is_the_major_minor_and_build_with_the_revision_after_it() {
        assert_eq!(
            windows_version(10, 0, "22631", Some(4317)),
            ("10.0.22631".to_string(), "22631.4317".to_string())
        );
        assert_eq!(
            windows_version(10, 0, "22631", None),
            ("10.0.22631".to_string(), "22631".to_string())
        );
        assert_eq!(
            windows_version(10, 0, "19045", Some(0)),
            ("10.0.19045".to_string(), "19045.0".to_string())
        );
    }

    #[test]
    fn the_linux_version_is_the_leading_numbers_of_the_kernel_release() {
        assert_eq!(
            linux_version("6.8.0-45-generic\n"),
            pair("6.8.0", "6.8.0-45-generic")
        );
        assert_eq!(
            linux_version("5.15.153.1-microsoft-standard-WSL2"),
            pair("5.15.153.1", "5.15.153.1-microsoft-standard-WSL2")
        );
        assert_eq!(linux_version("6.1"), pair("6.1", "6.1"));
        assert_eq!(linux_version("6.8.0+"), pair("6.8.0", "6.8.0+"));
        assert_eq!(linux_version("6"), pair("6", "6"));
        // a dot that no digit follows does not belong to the version
        assert_eq!(linux_version("6.8."), pair("6.8", "6.8."));
        for text in ["", "\n", "generic", "-6.8.0", ".6"] {
            assert_eq!(linux_version(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_model_is_what_the_machine_calls_itself_not_a_makers_placeholder() {
        assert_eq!(model("Mac15,6\n"), some("Mac15,6"));
        assert_eq!(
            model("Raspberry Pi 4 Model B Rev 1.4\0"),
            some("Raspberry Pi 4 Model B Rev 1.4")
        );
        assert_eq!(model("  Latitude 7440 \r\n"), some("Latitude 7440"));
        for placeholder in [
            "To Be Filled By O.E.M.",
            "System Product Name",
            "Default string",
            "None",
            "Not Specified",
            "to be filled by o.e.m.\n",
            "SYSTEM PRODUCT NAME",
            " none ",
        ] {
            assert_eq!(model(placeholder), None, "{placeholder:?}");
        }
        for empty in ["", " \n", "\0", "\0\0 \n"] {
            assert_eq!(model(empty), None, "{empty:?}");
        }
        // only the whole text is a placeholder
        assert_eq!(model("None of Them 5"), some("None of Them 5"));
    }

    #[test]
    fn a_text_that_begins_like_an_unfilled_template_is_no_model() {
        for template in [
            "To be filled",
            "To be set",
            "OEM",
            "O.E.M.",
            "System Product",
            "Unknown Product",
        ] {
            // as it is, in other cases, and with more after it
            for text in [
                template.to_string(),
                template.to_uppercase(),
                template.to_lowercase(),
                format!("{template} by O.E.M."),
                format!(" {} NAME\r\n", template.to_uppercase()),
            ] {
                assert_eq!(model(&text), None, "{text:?}");
            }
        }
    }

    #[test]
    fn a_word_that_stands_for_no_value_is_no_model_only_as_the_whole_text() {
        for word in [
            "None",
            "Default string",
            "Undefined",
            "Not Specified",
            "Not Applicable",
            "Not Defined",
            "Not Available",
            "INVALID",
            "Type1ProductConfigId",
            "TBD by OEM",
            "N/A",
            "Unknown",
            "System Name",
        ] {
            for text in [
                word.to_string(),
                word.to_uppercase(),
                word.to_lowercase(),
                format!(" {word}\r\n"),
            ] {
                assert_eq!(model(&text), None, "{text:?}");
            }
            let longer = format!("{word} 3");
            assert_eq!(model(&longer), some(&longer), "{longer:?}");
        }
    }

    #[test]
    fn real_models_are_models_even_when_they_look_like_a_filler() {
        for text in [
            "Latitude 7440",
            "Mac15,6",
            "Raspberry Pi 4 Model B Rev 1.4",
            "20XW00FEUS",
            "Standard PC (Q35 + ICH9, 2009)",
            "VMware Virtual Platform",
            "Virtual Machine",
            // begins with an `O`, not with `OEM`
            "OptiPlex 7010",
            // begins with a word that is a filler only on its own
            "Nonesuch 3",
            // begins like `System Product`
            "Systems Inc. Box",
        ] {
            assert_eq!(model(text), some(text), "{text:?}");
        }
    }

    #[test]
    fn the_end_of_a_prefix_may_fall_inside_a_character() {
        // none of these is a filler, and where a prefix would end is the
        // middle of a character in most of them: comparing must not panic
        for text in [
            // `OEM` and `SYSTEM` in full-width letters, three bytes each
            "\u{ff2f}\u{ff25}\u{ff2d}",
            "\u{ff33}\u{ff39}\u{ff33}\u{ff34}\u{ff25}\u{ff2d}",
            // a two-byte character where a prefix ends
            "OE\u{e9}",
            "O.E.M\u{e9}",
            "To be se\u{e9}",
            "To be fille\u{e9}",
            "System Produc\u{e9}",
            "Unknown Produc\u{e9}",
            // shorter than every prefix
            "\u{e9}",
        ] {
            assert_eq!(model(text), some(text), "{text:?}");
        }
    }

    #[test]
    fn a_posix_locale_becomes_a_language_tag() {
        let lang = |value: &str| posix_language(None, None, Some(value));
        assert_eq!(lang("zh_CN.UTF-8"), some("zh-CN"));
        assert_eq!(lang("en_US"), some("en-US"));
        assert_eq!(lang("sr_RS@latin"), some("sr-RS"));
        assert_eq!(lang("sr_RS.UTF-8@latin"), some("sr-RS"));
        assert_eq!(lang("de"), some("de"));
        // C and POSIX are no language, whatever follows them
        for value in ["C", "POSIX", "C.UTF-8", "POSIX.UTF-8", "", " ", ".UTF-8"] {
            assert_eq!(lang(value), None, "{value:?}");
        }
    }

    #[test]
    fn a_locale_name_stands_for_one_language_or_for_none() {
        for (locale, language) in [
            ("zh_CN.UTF-8", some("zh-CN")),
            ("en_US", some("en-US")),
            ("sr_RS.UTF-8@latin", some("sr-RS")),
            ("sr_RS@latin", some("sr-RS")),
            // the white space around a name is no part of it
            (" \tzh_CN.UTF-8\r\n", some("zh-CN")),
            // C and POSIX are no language, whatever follows them
            ("C", None),
            ("C.UTF-8", None),
            ("POSIX", None),
            // no name, or nothing before the encoding or the modifier
            ("", None),
            (" \t\r\n", None),
            ("@euro", None),
            (".UTF-8", None),
        ] {
            assert_eq!(language_of(locale), language, "{locale:?}");
        }
    }

    #[test]
    fn the_first_locale_variable_that_is_set_decides() {
        let all = |lc_all, lc_messages, lang| posix_language(lc_all, lc_messages, lang);
        assert_eq!(
            all(
                Some("fr_FR.UTF-8"),
                Some("de_DE.UTF-8"),
                Some("en_US.UTF-8")
            ),
            some("fr-FR")
        );
        assert_eq!(
            all(None, Some("de_DE.UTF-8"), Some("en_US.UTF-8")),
            some("de-DE")
        );
        assert_eq!(all(None, None, Some("en_US.UTF-8")), some("en-US"));
        // empty is as good as not set
        assert_eq!(all(Some(""), Some(""), Some("en_US.UTF-8")), some("en-US"));
        assert_eq!(all(None, None, None), None);
        // the one that decides may say C: that is the locale, not a reason to
        // look at the next one
        assert_eq!(all(Some("C"), None, Some("zh_CN.UTF-8")), None);
        assert_eq!(all(None, Some("POSIX"), Some("zh_CN.UTF-8")), None);
    }

    #[test]
    fn the_first_of_the_apple_languages_is_the_language() {
        assert_eq!(
            apple_languages("(\n    \"en-US\",\n    \"zh-Hans-US\"\n)\n"),
            some("en-US")
        );
        assert_eq!(apple_languages("(\n    \"ja-JP\"\n)\n"), some("ja-JP"));
        // older systems print the tags without quotes
        assert_eq!(
            apple_languages("(\n    en,\n    ja,\n    fr\n)\n"),
            some("en")
        );
        assert_eq!(apple_languages("(\n    en\n)"), some("en"));
        // and on one line, quoting the tags that have a hyphen
        assert_eq!(
            apple_languages("(en, \"zh-Hans\", \"zh-Hant\", ja, fr)"),
            some("en")
        );
        assert_eq!(apple_languages("(\"zh-Hans\", en)"), some("zh-Hans"));
        // no languages at all, and what is not a list
        for text in [
            "(\n)\n",
            "()",
            "",
            "\n",
            "en-US",
            "The domain/default pair of (kCFPreferencesAnyApplication, AppleLanguages) does not exist",
        ] {
            assert_eq!(apple_languages(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_name_is_the_text_without_its_white_space() {
        assert_eq!(name("build-box\n"), some("build-box"));
        assert_eq!(name("Tim's MacBook Pro\n"), some("Tim's MacBook Pro"));
        assert_eq!(name(" a b \r\n"), some("a b"));
        for empty in ["", " ", "\n", "\r\n"] {
            assert_eq!(name(empty), None, "{empty:?}");
        }
    }

    /// `reader` over fakes: the files and the environment there are, and the
    /// runner that answers.
    fn read_with(
        reader: fn(&Sources<'_>) -> SystemInfo,
        runner: &dyn CommandRunner,
        files: &[(&str, &str)],
        env: &[(&str, &str)],
    ) -> SystemInfo {
        let file = |path: &str| {
            files
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, text)| text.to_string())
        };
        let env = |name: &str| {
            env.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, value)| OsString::from(value))
        };
        reader(&Sources {
            runner,
            file: &file,
            env: &env,
        })
    }

    /// A Mac that answers every question.
    fn mac() -> FakeRunner {
        let runner = FakeRunner::default();
        runner.reply("sysctl -n hw.model", "Mac15,6\n");
        runner.reply(
            "defaults read -g AppleLanguages",
            "(\n    \"en-US\",\n    \"zh-Hans-US\"\n)\n",
        );
        runner.reply("scutil --get ComputerName", "Tim's MacBook Pro\n");
        runner
    }

    const MAC_FILES: [(&str, &str); 1] =
        [("/System/Library/CoreServices/SystemVersion.plist", PLIST)];

    fn mac_info() -> SystemInfo {
        SystemInfo {
            version: some("System Version 14.6.1 (Build 23G93)"),
            model: some("Mac15,6"),
            language: some("en-US"),
            name: some("Tim's MacBook Pro"),
        }
    }

    /// A system on which none of the tools exist.
    struct NoTools;

    impl CommandRunner for NoTools {
        fn run(&self, cmd: &[String]) -> io::Result<String> {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("cannot run {}", cmd[0]),
            ))
        }
    }

    #[test]
    fn macos_asks_each_source_for_its_value_and_nothing_else() {
        let runner = mac();
        assert_eq!(read_with(read_macos, &runner, &MAC_FILES, &[]), mac_info());
        assert_eq!(
            runner.calls(),
            [
                "sysctl -n hw.model",
                "defaults read -g AppleLanguages",
                "scutil --get ComputerName"
            ]
        );
    }

    #[test]
    fn macos_loses_only_the_value_whose_source_gives_nothing() {
        // the property list cannot be read, or is not one
        for files in [
            &[][..],
            &[("/System/Library/CoreServices/SystemVersion.plist", "nope")][..],
        ] {
            assert_eq!(
                read_with(read_macos, &mac(), files, &[]),
                SystemInfo {
                    version: None,
                    ..mac_info()
                }
            );
        }
        // each command fails, says nothing, or says what cannot be read
        let without = |command: &str, outcome: Result<&str, &str>| {
            let runner = mac();
            match outcome {
                Ok(stdout) => runner.reply(command, stdout),
                Err(message) => runner.fail(command, message),
            }
            read_with(read_macos, &runner, &MAC_FILES, &[])
        };
        for outcome in [
            Err("sysctl: unknown oid 'hw.model'"),
            Ok(""),
            Ok("\n"),
            Ok("\0"),
        ] {
            assert_eq!(
                without("sysctl -n hw.model", outcome),
                SystemInfo {
                    model: None,
                    ..mac_info()
                },
                "{outcome:?}"
            );
        }
        for outcome in [
            Err(
                "The domain/default pair of (kCFPreferencesAnyApplication, AppleLanguages) does not exist",
            ),
            Ok(""),
            Ok("(\n)\n"),
            Ok("not a list"),
        ] {
            assert_eq!(
                without("defaults read -g AppleLanguages", outcome),
                SystemInfo {
                    language: None,
                    ..mac_info()
                },
                "{outcome:?}"
            );
        }
    }

    #[test]
    fn macos_names_the_machine_by_its_local_host_name_when_it_has_no_computer_name() {
        for computer_name in [Err("ComputerName: not set"), Ok(""), Ok("\n")] {
            let runner = mac();
            match computer_name {
                Ok(stdout) => runner.reply("scutil --get ComputerName", stdout),
                Err(message) => runner.fail("scutil --get ComputerName", message),
            }
            runner.reply("scutil --get LocalHostName", "Tims-MacBook-Pro\n");
            let info = read_with(read_macos, &runner, &MAC_FILES, &[]);
            assert_eq!(
                info,
                SystemInfo {
                    name: some("Tims-MacBook-Pro"),
                    ..mac_info()
                },
                "{computer_name:?}"
            );
            assert_eq!(
                runner.calls(),
                [
                    "sysctl -n hw.model",
                    "defaults read -g AppleLanguages",
                    "scutil --get ComputerName",
                    "scutil --get LocalHostName"
                ]
            );
        }
        // with neither there is no name
        let runner = mac();
        runner.fail("scutil --get ComputerName", "ComputerName: not set");
        runner.fail("scutil --get LocalHostName", "LocalHostName: not set");
        assert_eq!(
            read_with(read_macos, &runner, &MAC_FILES, &[]),
            SystemInfo {
                name: None,
                ..mac_info()
            }
        );
    }

    #[test]
    fn macos_takes_the_language_from_the_environment_when_the_system_has_none() {
        let runner = mac();
        runner.fail("defaults read -g AppleLanguages", "does not exist");
        let language =
            |env: &[(&str, &str)]| read_with(read_macos, &runner, &MAC_FILES, env).language;
        assert_eq!(language(&[("LANG", "de_DE.UTF-8")]), some("de-DE"));
        assert_eq!(
            language(&[("LC_ALL", "fr_FR.UTF-8"), ("LANG", "de_DE.UTF-8")]),
            some("fr-FR")
        );
        assert_eq!(language(&[("LC_MESSAGES", "it_IT")]), some("it-IT"));
        assert_eq!(language(&[("LANG", "C")]), None);
        assert_eq!(language(&[]), None);
        // the system comes first when it has an answer
        assert_eq!(
            read_with(read_macos, &mac(), &MAC_FILES, &[("LANG", "de_DE.UTF-8")]).language,
            some("en-US")
        );
    }

    #[test]
    fn macos_without_any_of_its_tools_keeps_what_files_and_the_environment_give() {
        assert_eq!(
            read_with(read_macos, &NoTools, &MAC_FILES, &[("LANG", "de_DE.UTF-8")]),
            SystemInfo {
                version: some("System Version 14.6.1 (Build 23G93)"),
                model: None,
                language: some("de-DE"),
                name: None,
            }
        );
        assert_eq!(
            read_with(read_macos, &NoTools, &[], &[]),
            SystemInfo::default()
        );
    }

    #[test]
    fn linux_reads_files_and_the_environment_and_runs_no_command() {
        let runner = FakeRunner::default();
        let info = read_with(
            read_linux,
            &runner,
            &[
                ("/proc/sys/kernel/osrelease", "6.8.0-45-generic\n"),
                ("/sys/class/dmi/id/product_name", "Latitude 7440\n"),
                ("/proc/sys/kernel/hostname", "build-box\n"),
            ],
            &[("LANG", "en_US.UTF-8")],
        );
        assert_eq!(
            info,
            SystemInfo {
                version: some("System Version 6.8.0 (Build 6.8.0-45-generic)"),
                model: some("Latitude 7440"),
                language: some("en-US"),
                name: some("build-box"),
            }
        );
        assert!(runner.calls().is_empty(), "{:?}", runner.calls());
    }

    #[test]
    fn linux_asks_the_device_tree_for_a_model_when_dmi_has_none() {
        let model = |files: &[(&str, &str)]| {
            read_with(read_linux, &FakeRunner::default(), files, &[]).model
        };
        // a maker's placeholder is no model; the device tree ends in a NUL
        assert_eq!(
            model(&[
                ("/sys/class/dmi/id/product_name", "To Be Filled By O.E.M.\n"),
                (
                    "/proc/device-tree/model",
                    "Raspberry Pi 4 Model B Rev 1.4\0"
                ),
            ]),
            some("Raspberry Pi 4 Model B Rev 1.4")
        );
        // nor is an empty file, or a file that is not there
        assert_eq!(
            model(&[
                ("/sys/class/dmi/id/product_name", "\n"),
                (
                    "/proc/device-tree/model",
                    "Raspberry Pi 4 Model B Rev 1.4\0"
                ),
            ]),
            some("Raspberry Pi 4 Model B Rev 1.4")
        );
        assert_eq!(
            model(&[(
                "/proc/device-tree/model",
                "Raspberry Pi 4 Model B Rev 1.4\0"
            )]),
            some("Raspberry Pi 4 Model B Rev 1.4")
        );
        // DMI wins when it has a model
        assert_eq!(
            model(&[
                ("/sys/class/dmi/id/product_name", "Latitude 7440\n"),
                (
                    "/proc/device-tree/model",
                    "Raspberry Pi 4 Model B Rev 1.4\0"
                ),
            ]),
            some("Latitude 7440")
        );
        // with neither, or only placeholders, there is none
        assert_eq!(model(&[]), None);
        assert_eq!(
            model(&[
                ("/sys/class/dmi/id/product_name", "System Product Name\n"),
                ("/proc/device-tree/model", "\0"),
            ]),
            None
        );
    }

    #[test]
    fn linux_names_the_machine_by_the_first_host_name_it_finds() {
        let name = |files: &[(&str, &str)], env: &[(&str, &str)]| {
            read_with(read_linux, &FakeRunner::default(), files, env).name
        };
        assert_eq!(
            name(
                &[
                    ("/proc/sys/kernel/hostname", "kernel-name\n"),
                    ("/etc/hostname", "file-name\n")
                ],
                &[("HOSTNAME", "variable-name")]
            ),
            some("kernel-name")
        );
        assert_eq!(
            name(
                &[("/etc/hostname", "file-name\n")],
                &[("HOSTNAME", "variable-name")]
            ),
            some("file-name")
        );
        assert_eq!(
            name(&[], &[("HOSTNAME", "variable-name")]),
            some("variable-name")
        );
        // a file with nothing in it counts for nothing
        assert_eq!(
            name(
                &[
                    ("/proc/sys/kernel/hostname", "\n"),
                    ("/etc/hostname", "file-name\n")
                ],
                &[]
            ),
            some("file-name")
        );
        assert_eq!(name(&[], &[("HOSTNAME", "")]), None);
        assert_eq!(name(&[], &[]), None);
    }

    #[test]
    fn linux_reads_the_language_from_the_three_locale_variables() {
        let language =
            |env: &[(&str, &str)]| read_with(read_linux, &FakeRunner::default(), &[], env).language;
        assert_eq!(language(&[("LC_ALL", "fr_FR.UTF-8")]), some("fr-FR"));
        assert_eq!(language(&[("LC_MESSAGES", "de_DE.UTF-8")]), some("de-DE"));
        assert_eq!(language(&[("LANG", "en_US.UTF-8")]), some("en-US"));
        assert_eq!(
            language(&[
                ("LANG", "en_US.UTF-8"),
                ("LC_MESSAGES", "de_DE.UTF-8"),
                ("LC_ALL", "fr_FR.UTF-8"),
            ]),
            some("fr-FR")
        );
        assert_eq!(language(&[("LANG", "C.UTF-8")]), None);
        assert_eq!(language(&[]), None);
    }

    #[test]
    fn linux_that_can_read_nothing_says_nothing() {
        let runner = FakeRunner::default();
        assert_eq!(
            read_with(read_linux, &runner, &[], &[]),
            SystemInfo::default()
        );
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn a_file_that_cannot_be_read_is_not_there() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hostname");
        std::fs::write(&file, "build-box\n").unwrap();
        assert_eq!(
            read_file(file.to_str().unwrap()),
            Some("build-box\n".to_string())
        );
        assert_eq!(
            read_file(dir.path().join("missing").to_str().unwrap()),
            None
        );
        // a directory is no file, and bytes that are not text are not read
        assert_eq!(read_file(dir.path().to_str().unwrap()), None);
        let binary = dir.path().join("binary");
        std::fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(read_file(binary.to_str().unwrap()), None);
    }

    #[test]
    fn reading_this_system_twice_gives_the_same_answer() {
        let first = read();
        assert_eq!(read(), first);
        if let Some(version) = &first.version {
            assert!(version.starts_with("System Version "), "{version}");
            assert!(version.contains(" (Build "), "{version}");
        }
    }

    /// The key every Windows has its version in.
    #[cfg(windows)]
    const CURRENT_VERSION: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

    #[cfg(windows)]
    #[test]
    fn a_registry_string_that_is_not_there_or_not_a_string_is_nothing() {
        use windows_registry::LOCAL_MACHINE;
        let string = |path: &str, value: &str| registry_string(LOCAL_MACHINE, path, value);
        assert!(string(CURRENT_VERSION, "CurrentBuildNumber").is_some());
        // no such key, no such value, a number and (in a Windows as installed)
        // a list where a string is wanted
        assert_eq!(string(r"SOFTWARE\no-such-key", "CurrentBuildNumber"), None);
        assert_eq!(string(CURRENT_VERSION, "NoSuchValue"), None);
        assert_eq!(string(CURRENT_VERSION, "UBR"), None);
        assert_eq!(
            string(
                r"SYSTEM\CurrentControlSet\Control\Session Manager",
                "BootExecute"
            ),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_first_of_a_registry_list_is_not_the_empty_string_that_ends_it() {
        use windows_registry::LOCAL_MACHINE;
        let manager = r"SYSTEM\CurrentControlSet\Control\Session Manager";
        let first = |path: &str, value: &str| registry_first_string(LOCAL_MACHINE, path, value);
        // `BootExecute` is a list in a Windows as installed; a machine may
        // have emptied or removed it, and then the test has only the cases
        // below
        let list = LOCAL_MACHINE
            .open(manager)
            .ok()
            .and_then(|key| key.get_multi_string("BootExecute").ok())
            .unwrap_or_default();
        assert_eq!(
            first(manager, "BootExecute"),
            list.iter().find_map(|item| name(item)),
            "{list:?}"
        );
        // a string is a list of one
        assert_eq!(
            first(CURRENT_VERSION, "CurrentBuildNumber"),
            registry_string(LOCAL_MACHINE, CURRENT_VERSION, "CurrentBuildNumber")
        );
        // no such key, no such value, a number
        assert_eq!(first(r"SOFTWARE\no-such-key", "BootExecute"), None);
        assert_eq!(first(manager, "NoSuchValue"), None);
        assert_eq!(first(CURRENT_VERSION, "UBR"), None);
    }

    /// Windows keeps its version in the registry, macOS and Linux in a file;
    /// a system that cannot be read for its version is a reader that is wrong.
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    #[test]
    fn this_system_says_its_version() {
        let version = read().version.expect("the version of this system is read");
        assert!(version.starts_with("System Version "), "{version}");
        assert!(version.contains(" (Build "), "{version}");
    }

    #[cfg(windows)]
    #[test]
    fn windows_says_the_name_of_the_machine() {
        assert!(read().name.is_some(), "the registry has the computer name");
    }
}
