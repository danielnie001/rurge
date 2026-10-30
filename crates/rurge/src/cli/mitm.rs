//! `rurge mitm ca generate` / `export`: the CA that MITM signs with
//! (`phase 4 M2 design 13`).
//!
//! `generate` makes a new CA and writes the four files a profile and the
//! clients use; `export` gives the certificate of the CA a profile already
//! names, as PEM on the terminal or as a file. Neither prints the passphrase,
//! the PKCS#12 or the private key (`phase 4 M2 design 15`).

use super::control::load_profile;
use clap::{Args, Subcommand};
use rurge_config::config::Platform;
use rurge_http::mitm::{Authority, Generated, certificate_pem, generate};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The certificate, in PEM: what a client trusts.
pub(crate) const CA_PEM: &str = "rurge-ca.pem";
/// The same certificate in DER: what some importers want instead.
pub(crate) const CA_CER: &str = "rurge-ca.cer";
/// The certificate and its private key, in PKCS#12.
pub(crate) const CA_P12: &str = "rurge-ca.p12";
/// The `[MITM]` section that names the PKCS#12 and its passphrase.
pub(crate) const CA_CONF: &str = "rurge-mitm.conf";

#[derive(Args)]
pub struct MitmArgs {
    #[command(subcommand)]
    pub command: MitmCommand,
}

#[derive(Subcommand)]
pub enum MitmCommand {
    /// The CA that MITM signs with
    Ca(CaArgs),
}

#[derive(Args)]
pub struct CaArgs {
    #[command(subcommand)]
    pub command: CaCommand,
}

#[derive(Subcommand)]
pub enum CaCommand {
    /// Make a new CA and write its four files into a directory
    Generate(GenerateArgs),
    /// Print or write the certificate of the CA a profile names
    Export(ExportArgs),
}

#[derive(Args)]
pub struct GenerateArgs {
    /// Directory for the four files; it must already exist
    #[arg(long, value_name = "DIR")]
    pub out: PathBuf,
}

#[derive(Args)]
pub struct ExportArgs {
    /// Profile whose [MITM] section names the CA
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    pub config: PathBuf,
    /// Write the certificate to this file instead of printing it
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
    /// Write the certificate as DER instead of PEM
    #[arg(long, requires = "out")]
    pub der: bool,
    /// Evaluate the profile as if running on this platform
    #[arg(long, value_parser = super::check::parse_platform)]
    pub platform: Option<Platform>,
    /// Data directory the profile's downloaded sources are read from: a
    /// managed profile names its [MITM] in the copy kept there (default:
    /// the platform data dir)
    #[arg(long, env = "RURGE_DATA_DIR", value_name = "DIR")]
    pub data_dir: Option<PathBuf>,
}

pub fn run(args: MitmArgs) -> anyhow::Result<ExitCode> {
    match args.command {
        MitmCommand::Ca(ca) => match ca.command {
            CaCommand::Generate(args) => generate_ca(args),
            CaCommand::Export(args) => export(args),
        },
    }
}

/// ① `--out` is a directory that is there → ② none of the four files is
/// there → ③ the CA → ④ the four files, only new ones → ⑤ the PKCS#12 read
/// back → ⑥ what to do with them (`the phase 4 / M2c plan's P6`).
///
/// ① and ② are errors of what was asked for and exit 2 with nothing written;
/// ③ ～ ⑤ are failures of the work, exit 1, and leave no file of this run
/// behind.
fn generate_ca(args: GenerateArgs) -> anyhow::Result<ExitCode> {
    let out = &args.out;
    if !out.is_dir() {
        anyhow::bail!(
            "{} is not a directory; rurge does not create it",
            out.display()
        );
    }
    let paths = [
        out.join(CA_PEM),
        out.join(CA_CER),
        out.join(CA_P12),
        out.join(CA_CONF),
    ];
    for path in &paths {
        // whatever is there counts, a broken symbolic link too
        if std::fs::symlink_metadata(path).is_ok() {
            anyhow::bail!(
                "{} already exists; rurge does not overwrite it",
                path.display()
            );
        }
    }

    let ca = match generate() {
        Ok(ca) => ca,
        Err(e) => {
            eprintln!("error: {e}");
            return Ok(ExitCode::from(1));
        }
    };
    let files: [(PathBuf, &[u8], bool); 4] = [
        (paths[0].clone(), ca.pem.as_bytes(), false),
        (paths[1].clone(), ca.der.as_slice(), false),
        (paths[2].clone(), ca.p12.as_slice(), true),
        (paths[3].clone(), ca.conf.as_bytes(), true),
    ];
    if let Err(e) = write_new(&files) {
        // `write_new` has removed what it wrote; its message names the file
        // that failed
        eprintln!("error: cannot write {e}");
        return Ok(ExitCode::from(1));
    }
    let written: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
    let p12 = match std::fs::read(&paths[2]) {
        Ok(p12) => p12,
        Err(e) => {
            remove_files(&written);
            eprintln!(
                "error: {} cannot be read back: {e}; the files written are removed",
                paths[2].display()
            );
            return Ok(ExitCode::from(1));
        }
    };
    if let Err(problem) = ca.check(&p12) {
        remove_files(&written);
        eprintln!(
            "error: {} does not read back as a CA rurge can sign with: {}; the files written are removed",
            paths[2].display(),
            problem.message
        );
        return Ok(ExitCode::from(1));
    }
    announce(&ca, out);
    Ok(ExitCode::SUCCESS)
}

/// The certificate of the CA the profile names: on the terminal as PEM, or in
/// the file `--out` gives, PEM or DER (`the phase 4 / M2c plan's P7`).
///
/// Only `[MITM]` and `[Keystore]` are read; a problem elsewhere in the profile
/// is not this command's to report.
fn export(args: ExportArgs) -> anyhow::Result<ExitCode> {
    let loaded = load_profile(&args.config, args.platform, args.data_dir.as_deref())?;
    let der = match Authority::load(&loaded.config.mitm, &loaded.config.keystore) {
        Ok(Some(authority)) => authority.ca_der().to_vec(),
        Ok(None) => anyhow::bail!("the profile's [MITM] names no CA (ca-p12 or ca-keystore-name)"),
        Err(problem) => return Err(anyhow::Error::msg(problem.message)),
    };
    match &args.out {
        None => {
            print!("{}", certificate_pem(&der));
            Ok(ExitCode::SUCCESS)
        }
        Some(path) => write_certificate(path, &der, args.der),
    }
}

/// Writes the certificate to `path`, which must not be there yet. A file that
/// cannot be finished is removed again (`the phase 4 / M2c plan's P7`).
fn write_certificate(path: &Path, der: &[u8], as_der: bool) -> anyhow::Result<ExitCode> {
    if std::fs::symlink_metadata(path).is_ok() {
        anyhow::bail!(
            "{} already exists; rurge does not overwrite it",
            path.display()
        );
    }
    let bytes = if as_der {
        der.to_vec()
    } else {
        certificate_pem(der).into_bytes()
    };
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("error: cannot write {}: {e}", path.display());
            return Ok(ExitCode::from(1));
        }
    };
    let result = file.write_all(&bytes);
    drop(file);
    if let Err(e) = result {
        let _ = std::fs::remove_file(path);
        eprintln!("error: cannot write {}: {e}", path.display());
        return Ok(ExitCode::from(1));
    }
    println!("wrote the CA certificate to {}", path.display());
    Ok(ExitCode::SUCCESS)
}

/// Writes the files, each only where nothing is yet: a failure removes every
/// file this call made and leaves the paths that were there alone
/// (`the phase 4 / M2c plan's P6`).
///
/// The error carries the path of the file that failed, which an `io::Error`
/// does not; the caller prints it as `cannot write {error}`.
fn write_new(files: &[(PathBuf, &[u8], bool)]) -> std::io::Result<()> {
    let mut written: Vec<&Path> = Vec::new();
    for (path, bytes, private) in files {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        restrict(&mut options, *private);
        let mut file = match options.open(path) {
            Ok(file) => file,
            Err(e) => {
                // `open` made nothing: this path is not this call's to remove
                remove_files(&written);
                return Err(named(path, e));
            }
        };
        written.push(path.as_path());
        let result = file.write_all(bytes);
        drop(file);
        if let Err(e) = result {
            remove_files(&written);
            return Err(named(path, e));
        }
    }
    Ok(())
}

/// `failed`, with the path it happened at in front of its message.
fn named(path: &Path, failed: std::io::Error) -> std::io::Error {
    std::io::Error::new(failed.kind(), format!("{}: {failed}", path.display()))
}

/// Removes the files a failed call made. An error here is not the reason the
/// call failed and does not stand in for it.
fn remove_files(paths: &[&Path]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

/// On Unix a private file is its owner's alone (phase 4 M2 design 13).
#[cfg(unix)]
fn restrict(options: &mut std::fs::OpenOptions, private: bool) {
    use std::os::unix::fs::OpenOptionsExt;
    if private {
        options.mode(0o600);
    }
}

/// Windows: rurge sets no permissions of its own; the folder's are the
/// file's (M2-D20).
#[cfg(not(unix))]
fn restrict(_options: &mut std::fs::OpenOptions, _private: bool) {}

/// What `generate` says once the files are in place: which four they are,
/// what to copy into a profile, and how to trust the CA (`rurge` does not
/// install it). No passphrase, no PKCS#12 and no key is printed
/// (`phase 4 M2 design 13`, `15`).
fn announce(ca: &Generated, out: &Path) {
    println!(
        "wrote a new MITM CA, \"{}\", valid until {}:",
        ca.common_name,
        ca.not_after.date()
    );
    for (name, what) in [
        (CA_PEM, "the certificate (PEM), to be trusted"),
        (CA_CER, "the same certificate (DER)"),
        (CA_P12, "the certificate and its private key (PKCS#12)"),
        (
            CA_CONF,
            "a [MITM] section with that PKCS#12 and its passphrase",
        ),
    ] {
        // the descriptions line up: the longest name is rurge-mitm.conf
        let pad = " ".repeat(CA_CONF.len() - name.len());
        println!("  {}{pad}  {what}", out.join(name).display());
    }
    println!();
    if cfg!(unix) {
        println!(
            "rurge-ca.p12 and rurge-mitm.conf hold the private key and its passphrase: they were made readable by you alone (0600); keep them that way."
        );
    } else {
        println!(
            "rurge-ca.p12 and rurge-mitm.conf hold the private key and its passphrase: rurge sets no permissions of its own on Windows, so keep them in a folder only you can read."
        );
    }
    println!();
    println!(
        "to use it: copy the lines of rurge-mitm.conf into your profile (they are a whole [MITM] section; if the profile has one, put ca-p12 and ca-passphrase in it instead), list the hosts to decrypt in hostname, and reload."
    );
    println!();
    println!("to trust it (rurge does not install it; run these where the files are):");
    for line in [
        "  Windows, this user:  certutil -user -addstore Root rurge-ca.cer",
        "  macOS, every user:   sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain rurge-ca.pem",
        "  Debian, Ubuntu:      sudo cp rurge-ca.pem /usr/local/share/ca-certificates/rurge-ca.crt && sudo update-ca-certificates",
        "  Fedora, RHEL:        sudo cp rurge-ca.pem /etc/pki/ca-trust/source/anchors/rurge-ca.pem && sudo update-ca-trust",
        "  Arch Linux:          sudo trust anchor --store rurge-ca.pem",
        "  NSS (Chrome and others on Linux): certutil -d sql:$HOME/.pki/nssdb -A -t \"C,,\" -n \"rurge MITM CA\" -i rurge-ca.pem",
        "  Firefox: Settings > Privacy & Security > Certificates > View Certificates > Authorities > Import rurge-ca.pem, and trust it to identify websites",
        "  Android: Settings > Security > Encryption & credentials > Install a certificate > CA certificate > rurge-ca.cer (apps trust it only if they opt in to user CAs)",
        "  iOS, iPadOS: open rurge-ca.cer on the device, install the profile in Settings, then turn on full trust in Settings > General > About > Certificate Trust Settings",
    ] {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `write_new` writes what it can and removes what it wrote when a later
    /// file cannot be made: the paths that are already there are not its to
    /// remove (`the phase 4 / M2c plan's P6`).
    #[test]
    fn the_files_written_are_removed_when_one_cannot_be() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let taken = dir.path().join("taken");
        let third = dir.path().join("third");
        std::fs::create_dir(&taken).unwrap();

        let result = write_new(&[
            (first.clone(), b"one", false),
            (taken.clone(), b"two", false),
            (third.clone(), b"three", false),
        ]);

        assert!(result.is_err(), "a directory where a file goes is an error");
        assert!(
            !first.exists(),
            "the file written before the failure is gone"
        );
        assert!(!third.exists(), "the file after the failure is not made");
        assert!(
            taken.is_dir(),
            "a directory that was there is not this call's to remove"
        );
    }
}
