//! `wintun.dll` (M4 design 8.4): where it is, who signed it, and how it is
//! loaded — the part of opening the Windows adapter that comes before the
//! adapter itself.
//!
//! The file is opened shared for reading only before its signature is checked
//! and stays open until the module is loaded (plan P7): what is checked is the
//! open file (`WINTRUST_FILE_INFO`'s `hFile`), and nobody can write to it,
//! delete it or rename it in between. The signer has to be exactly
//! [`SIGNER`] (M4-D13), and checking never goes to the network (P6). The
//! module is loaded from the path alone, and its dependencies come from
//! System32 only (P7).
//!
//! Once loaded it stays loaded for the life of the process: the entry points
//! in [`Wintun`] are taken once and stay valid, and nothing is ever unloaded.

use std::ffi::{OsStr, c_void};
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::NetworkManagement::Ndis::NET_LUID_LH;
use windows_sys::core::GUID;

/// The signer the file has to have (M4-D13): the Wintun project's own
/// certificate.
const SIGNER: &str = "WireGuard LLC";

/// `FILE_SHARE_READ` (in `Win32_Storage_FileSystem`, `0x1`): others may read
/// the file while it is open, and may not write to it, delete it or rename it.
/// Defined here rather than opening that feature for one constant (plan P2,
/// execution record row 3).
const FILE_SHARE_READ: u32 = 0x1;

/// What the error says when the file is not there: where to get it, where it
/// goes, and which option says somewhere else (plan task 3).
const NOT_FOUND: &str = "cannot find wintun.dll (next to rurge.exe, or where --wintun-dll \
                         points): get it from https://www.wintun.net/";

/// The module this process has loaded, and the path it came from: at most one,
/// never unloaded (M4 design 8.4).
///
/// The path is part of the answer so that a second path is not answered with
/// the first file's module: one run has one path, and the tests of this module
/// would otherwise depend on each other's order.
static LOADED: Mutex<Option<(PathBuf, Arc<Wintun>)>> = Mutex::new(None);

/// `WINTUN_ADAPTER_HANDLE`: Wintun's own adapter object, null when it failed.
/// Kept as an integer by whoever holds it (plan P4).
type AdapterHandle = *mut c_void;

/// `WINTUN_SESSION_HANDLE`: a session on an adapter, null when it failed.
type SessionHandle = *mut c_void;

/// What `GetProcAddress` answers before an entry point has its own shape: the
/// same pointer, as `FARPROC` has it.
type Entry = unsafe extern "system" fn() -> isize;

/// A loaded wintun.dll: its eleven entry points, typed as `wintun.h` has them
/// (execution record row 6), and the module they came from.
///
/// The pointers are [`Send`] and [`Sync`] like any function pointer, which is
/// why the module handle is an integer: a pointer is neither (P4). Nothing is
/// unloaded, so they are valid for as long as this is reachable.
// The entry points are read from task 5 on (the phase 3 / M4d plan's
// execution record row 55); until then no field but `module` has a reader.
#[allow(dead_code)]
pub(super) struct Wintun {
    /// `LoadLibraryExW`'s handle as an integer (P4). Never freed.
    pub(super) module: usize,
    pub(super) create_adapter:
        unsafe extern "system" fn(*const u16, *const u16, *const GUID) -> AdapterHandle,
    pub(super) open_adapter: unsafe extern "system" fn(*const u16) -> AdapterHandle,
    pub(super) close_adapter: unsafe extern "system" fn(AdapterHandle),
    /// `WintunGetAdapterLUID`: the adapter's LUID, which the index of the
    /// adapter's interface comes from.
    pub(super) adapter_luid: unsafe extern "system" fn(AdapterHandle, *mut NET_LUID_LH),
    pub(super) start_session: unsafe extern "system" fn(AdapterHandle, u32) -> SessionHandle,
    pub(super) end_session: unsafe extern "system" fn(SessionHandle),
    /// `WintunGetReadWaitEvent`: the session's read event, which is why the
    /// reader waits on it (M4 design 8.4).
    pub(super) read_event: unsafe extern "system" fn(SessionHandle) -> HANDLE,
    /// `WintunReceivePacket`: a packet, or null with an error code; the size
    /// comes back through the second argument.
    pub(super) receive: unsafe extern "system" fn(SessionHandle, *mut u32) -> *mut u8,
    pub(super) release_receive: unsafe extern "system" fn(SessionHandle, *const u8),
    /// `WintunAllocateSendPacket`: room for a packet to be written into, or
    /// null when the ring is full.
    pub(super) allocate_send: unsafe extern "system" fn(SessionHandle, u32) -> *mut u8,
    pub(super) send: unsafe extern "system" fn(SessionHandle, *const u8),
}

/// Opening's first part (M4 design 8.4), and the only way the module is
/// loaded: the file at `path` has to be a Wintun signed by [`SIGNER`], and the
/// loaded module stays for the life of the process.
///
/// The file has to be there (`NotFound`), the signer has to be right
/// (`PermissionDenied` when it is not), and the module has to be a Wintun
/// (`InvalidData` when an entry point is missing). A failed attempt is not
/// remembered: the next open tries again.
///
/// Any other failure is the system's, with what it is about in front of it
/// (`cannot open wintun.dll: …`, `cannot load wintun.dll: …`) and the kind the
/// system gave. The words `frame::explained` puts in front of a failed open
/// stand for the adapter, not for these, so the caller passes these on as they
/// are (the plan's execution record row 59). No message names the path (P16).
// Read from task 5 on (the phase 3 / M4d plan's execution record row 55).
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn loaded(path: &Path) -> io::Result<Arc<Wintun>> {
    {
        let loaded = LOADED.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((from, wintun)) = loaded.as_ref()
            && from == path
        {
            return Ok(Arc::clone(wintun));
        }
    }
    tracing::debug!(path = %path.display(), "wintun.dll");
    // shared for reading only, so that the file cannot be written, deleted or
    // renamed while it is checked and loaded (P7)
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => io::Error::new(io::ErrorKind::NotFound, NOT_FOUND),
            kind => io::Error::new(kind, format!("cannot open wintun.dll: {error}")),
        })?;
    // the file stays open until the module is loaded: what is checked is this
    // file, and nothing can be swapped for it in between
    match verify_signer(path, &file) {
        Ok(Some(signer)) if signer == SIGNER => {}
        Ok(Some(signer)) => {
            return Err(not_ours(format!(
                "wintun.dll is not signed by {SIGNER} (it is signed by {signer})"
            )));
        }
        Ok(None) => return Err(not_ours("wintun.dll is not signed".to_string())),
        Err(text) => return Err(not_ours(text)),
    }
    let wintun = Arc::new(load(path)?);
    *LOADED.lock().unwrap_or_else(PoisonError::into_inner) =
        Some((path.to_path_buf(), Arc::clone(&wintun)));
    Ok(wintun)
}

/// The simple display name of the signer of `file`, opened from `path` (M4
/// design 8.4): `WinVerifyTrust` on the open file
/// (`WINTRUST_ACTION_GENERIC_VERIFY_V2`, no UI, `WTD_REVOKE_NONE`,
/// `WTD_CACHE_ONLY_URL_RETRIEVAL`: never the network), the signer's
/// certificate from the state data, and `CertGetNameStringW`
/// (`CERT_NAME_SIMPLE_DISPLAY_TYPE`).
///
/// `Ok(None)` is a file with no signature (`TRUST_E_NOSIGNATURE`); any other
/// answer that is not 0 is `wintun.dll's signature cannot be trusted` with the
/// result in hexadecimal. The state the verification opened is closed on the
/// way out, whatever happened (the `WINTRUST_DATA` documentation: every
/// `VERIFY` needs a `CLOSE`).
#[allow(unsafe_code)]
fn verify_signer(path: &Path, file: &File) -> Result<Option<String>, String> {
    use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, TRUST_E_NOSIGNATURE};
    use windows_sys::Win32::Security::Cryptography::{
        CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW,
    };
    use windows_sys::Win32::Security::WinTrust::{
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
        WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
        WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvSignerFromChain,
        WTHelperProvDataFromStateData, WinVerifyTrust,
    };

    let path = wide(path.as_os_str())?;
    let mut info = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: path.as_ptr(),
        // the file that is open, not a second one opened by the path: this is
        // the file the signature is checked on (P7)
        hFile: file.as_raw_handle() as HANDLE,
        // no known subject: the file's own signature is what is checked
        pgKnownSubject: std::ptr::null_mut(),
    };
    let mut data = WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 {
            pFile: &raw mut info,
        },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL,
        ..WINTRUST_DATA::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: `hwnd` is the window a user interface would be shown in, and
    // `INVALID_HANDLE_VALUE` is what the function's documentation gives for a
    // call with no interactive user (`WTD_UI_NONE` says none is wanted);
    // `action` is the generic verification's own GUID, and `data` and the
    // `info` its union points at outlive the call.
    let result =
        unsafe { WinVerifyTrust(INVALID_HANDLE_VALUE, &mut action, (&raw mut data).cast()) };
    let answer = if result == TRUST_E_NOSIGNATURE {
        Ok(None)
    } else if result != 0 {
        Err(untrusted(result))
    } else {
        // the signer's name, from the state data this call just left: read it
        // before the state is closed below
        let name = || -> Result<String, String> {
            // SAFETY: the state handle is the one the `VERIFY` call returned,
            // and the state is closed only after this closure has run.
            let provider = unsafe { WTHelperProvDataFromStateData(data.hWVTStateData) };
            if provider.is_null() {
                return Err(no_signer());
            }
            // SAFETY: signer 0 of that provider data, not its counter signer.
            let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, 0, 0) };
            if signer.is_null() {
                return Err(no_signer());
            }
            // SAFETY: the structure the call returned, valid until the state
            // is closed.
            let (chain, count) = unsafe { ((*signer).pasCertChain, (*signer).csCertChain) };
            if chain.is_null() || count == 0 {
                return Err(no_signer());
            }
            // SAFETY: the first certificate of a chain of at least one.
            let cert = unsafe { (*chain).pCert };
            if cert.is_null() {
                return Err(no_signer());
            }
            // SAFETY: a certificate of that chain, a null parameter for this
            // name type, and no buffer: the answer is the length the name
            // needs, its terminator included.
            let needed = unsafe {
                CertGetNameStringW(
                    cert,
                    CERT_NAME_SIMPLE_DISPLAY_TYPE,
                    0,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    0,
                )
            };
            if needed <= 1 {
                return Err(no_signer());
            }
            let mut name = vec![0u16; needed as usize];
            // SAFETY: the buffer holds the `needed` units the call was told
            // about, and outlives the call.
            let written = unsafe {
                CertGetNameStringW(
                    cert,
                    CERT_NAME_SIMPLE_DISPLAY_TYPE,
                    0,
                    std::ptr::null(),
                    name.as_mut_ptr(),
                    needed,
                )
            };
            if written == 0 || written > needed {
                return Err(no_signer());
            }
            // the length the second call answers with has its terminator in it
            Ok(String::from_utf16_lossy(&name[..(written - 1) as usize]))
        };
        name().map(Some)
    };
    // SAFETY: the state data the `VERIFY` call above opened, closed with the
    // same structure and action id: the documented end of every verification,
    // whatever its result was.
    unsafe {
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        WinVerifyTrust(INVALID_HANDLE_VALUE, &mut action, (&raw mut data).cast());
    }
    answer
}

/// Loads `wintun.dll` from `path` (M4 design 8.4): its dependencies from
/// System32 only, and its eleven entry points. A module that has no Wintun in
/// it is freed again before the error is returned, which names the entry point
/// that was missing and asks whether this is wintun.dll at all.
#[allow(unsafe_code)]
fn load(path: &Path) -> io::Result<Wintun> {
    use windows_sys::Win32::Foundation::FreeLibrary;
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
    };

    let path =
        wide(path.as_os_str()).map_err(|text| io::Error::new(io::ErrorKind::InvalidInput, text))?;
    // SAFETY: the path is NUL-terminated UTF-16 and outlives the call; no file
    // is given, the loader opens the DLL itself; the flag makes the DLL's own
    // dependencies come from System32 only (P7).
    let module = unsafe {
        LoadLibraryExW(
            path.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if module.is_null() {
        let error = io::Error::last_os_error();
        return Err(io::Error::new(
            error.kind(),
            format!("cannot load wintun.dll: {error}"),
        ));
    }
    // one entry point of wintun.h's shape, or the module is freed and the entry
    // point that was missing is named
    macro_rules! entry {
        ($name:literal, $type:ty) => {{
            // SAFETY: a plain call with the module handle above and the
            // NUL-terminated name of a function it may export; the answer is
            // that function's address, or null.
            let address = unsafe { GetProcAddress(module, concat!($name, "\0").as_ptr()) };
            let Some(address) = address else {
                // SAFETY: the handle the load above answered, which nothing
                // else holds and nothing uses after this free: the entry
                // points are only used once all of them were found.
                unsafe { FreeLibrary(module) };
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("wintun.dll has no {}: is it wintun.dll?", $name),
                ));
            };
            // SAFETY: every entry point of wintun.h has the calling convention
            // and the arguments this type gives it (row 6), and the address is
            // that entry point's, so no other function is ever called through
            // it.
            unsafe { std::mem::transmute::<Entry, $type>(address) }
        }};
    }
    Ok(Wintun {
        module: module as usize,
        create_adapter: entry!(
            "WintunCreateAdapter",
            unsafe extern "system" fn(*const u16, *const u16, *const GUID) -> AdapterHandle
        ),
        open_adapter: entry!(
            "WintunOpenAdapter",
            unsafe extern "system" fn(*const u16) -> AdapterHandle
        ),
        close_adapter: entry!(
            "WintunCloseAdapter",
            unsafe extern "system" fn(AdapterHandle)
        ),
        adapter_luid: entry!(
            "WintunGetAdapterLUID",
            unsafe extern "system" fn(AdapterHandle, *mut NET_LUID_LH)
        ),
        start_session: entry!(
            "WintunStartSession",
            unsafe extern "system" fn(AdapterHandle, u32) -> SessionHandle
        ),
        end_session: entry!("WintunEndSession", unsafe extern "system" fn(SessionHandle)),
        read_event: entry!(
            "WintunGetReadWaitEvent",
            unsafe extern "system" fn(SessionHandle) -> HANDLE
        ),
        receive: entry!(
            "WintunReceivePacket",
            unsafe extern "system" fn(SessionHandle, *mut u32) -> *mut u8
        ),
        release_receive: entry!(
            "WintunReleaseReceivePacket",
            unsafe extern "system" fn(SessionHandle, *const u8)
        ),
        allocate_send: entry!(
            "WintunAllocateSendPacket",
            unsafe extern "system" fn(SessionHandle, u32) -> *mut u8
        ),
        send: entry!(
            "WintunSendPacket",
            unsafe extern "system" fn(SessionHandle, *const u8)
        ),
    })
}

/// `text` as Windows takes a path: UTF-16 with a NUL at the end. A NUL that is
/// already in it would end the path early, so that is refused.
fn wide(text: &OsStr) -> Result<Vec<u16>, String> {
    let mut units: Vec<u16> = text.encode_wide().collect();
    if units.contains(&0) {
        return Err("the path of wintun.dll has a NUL in it".to_string());
    }
    units.push(0);
    Ok(units)
}

/// What a refused verification says: its result as Windows gives it, in eight
/// hexadecimal digits. The path is not in it (plan P16).
fn untrusted(result: i32) -> String {
    format!(
        "wintun.dll's signature cannot be trusted (0x{:08X})",
        result as u32
    )
}

/// What a verification that cannot name its signer says, in [`untrusted`]'s
/// words.
fn no_signer() -> String {
    "wintun.dll's signature cannot be trusted (no signer)".to_string()
}

/// A refusal the user can act on: this file is not the Wintun rurge trusts.
fn not_ours(text: String) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The test binary is not signed, so it stands in for a `wintun.dll`
    /// somebody put there: the signature check has to refuse it before
    /// anything is loaded. The binary itself is not touched, only a copy.
    #[test]
    fn an_unsigned_file_is_refused() {
        let exe = std::env::current_exe().expect("the test binary's own path");
        let file = File::open(&exe).expect("the test binary opens");
        assert_eq!(
            verify_signer(&exe, &file),
            Ok(None),
            "the test binary has no signature to check"
        );

        let dir = tempfile::tempdir().expect("a temporary directory");
        let copy = dir.path().join("wintun.dll");
        fs::copy(&exe, &copy).expect("a copy of the test binary");

        let Err(error) = loaded(&copy) else {
            panic!("an unsigned file was loaded as wintun.dll");
        };
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "wintun.dll is not signed");
    }

    /// A file that is not there says where to get one.
    #[test]
    fn a_missing_file_says_where_to_get_it() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let missing = dir.path().join("wintun.dll");

        let Err(error) = loaded(&missing) else {
            panic!("a file that is not there was loaded");
        };
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            error.to_string(),
            "cannot find wintun.dll (next to rurge.exe, or where --wintun-dll points): \
             get it from https://www.wintun.net/"
        );
    }

    /// A DLL that is not Wintun is refused by the entry point it has not got.
    /// `kernel32.dll` is in every process already, so loading it adds one
    /// reference, and freeing it takes that one away again.
    #[test]
    fn a_dll_without_the_wintun_entry_points_is_refused() {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        let kernel32 = Path::new(&root).join("System32").join("kernel32.dll");

        let Err(error) = load(&kernel32) else {
            panic!("kernel32.dll was loaded as wintun.dll");
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            error.to_string(),
            "wintun.dll has no WintunCreateAdapter: is it wintun.dll?"
        );
    }

    /// A file that is no DLL at all is refused by the loader, and the error
    /// says what failed with the system's own words.
    #[test]
    fn a_file_that_is_not_a_dll_cannot_be_loaded() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let text = dir.path().join("not-a-dll.dll");
        fs::write(&text, b"this is not a PE image").expect("a text file");

        let Err(error) = load(&text) else {
            panic!("a text file was loaded as a DLL");
        };
        assert!(
            error.to_string().starts_with("cannot load wintun.dll: "),
            "{error}"
        );
    }

    /// A file somebody else has open without sharing cannot be opened, and the
    /// error says so with the system's own words.
    #[test]
    fn a_file_that_is_held_open_cannot_be_opened() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let held = dir.path().join("wintun.dll");
        fs::write(&held, b"held open by somebody else").expect("a file to hold");
        // shared with nothing at all: a second open of the same file is
        // refused, as it is for a wintun.dll somebody has open for writing
        let _held = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&held)
            .expect("the test holds the file");

        let Err(error) = loaded(&held) else {
            panic!("a file nobody else can open was opened");
        };
        assert!(
            error.to_string().starts_with("cannot open wintun.dll: "),
            "{error}"
        );
        assert_ne!(error.kind(), io::ErrorKind::NotFound, "the file is there");
    }

    /// A real `wintun.dll`, when the run says where one is
    /// (`RURGE_TEST_WINTUN_DLL`): signed by WireGuard LLC, and loaded. Nothing
    /// is made on it: no adapter, no session (M4d plan P19).
    #[test]
    fn a_real_wintun_dll_passes() {
        let Ok(path) = std::env::var("RURGE_TEST_WINTUN_DLL") else {
            eprintln!(
                "RURGE_TEST_WINTUN_DLL is not set: there is no wintun.dll to check \
                 (see docs/testing.md), so this test does nothing"
            );
            return;
        };

        let wintun = loaded(Path::new(&path)).expect("a signed wintun.dll is loaded");
        assert_ne!(wintun.module, 0, "the module handle is the load's answer");
    }
}
