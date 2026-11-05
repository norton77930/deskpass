//! `app_identity` (ticket 93): who an app is, for its approval and its always-allow grant.
//!
//! A packaged (UWP or MSIX) app is its package family name; Windows verified the package's
//! signature when it was installed, so no Authenticode check is made (spec-phase4b). Any other
//! executable is checked with `WinVerifyTrust` (WINTRUST_ACTION_GENERIC_VERIFY_V2, which builds and
//! checks the certificate chain): its embedded signature first, then the system catalogs, which
//! sign most Windows binaries (notepad.exe has no embedded signature). The signer is the leaf
//! certificate's full subject in X.500 form, in the order the certificate encodes it
//! (`C=US, S=Washington, L=Redmond, O=Microsoft Corporation, CN=Microsoft Windows`), which a vendor
//! keeps across updates and certificate renewals; the simple display name (the CN alone) would be
//! weaker.
//!
//! Revocation is not checked (`WTD_REVOKE_NONE`, `WTD_REVOCATION_CHECK_NONE`), and every URL
//! retrieval comes from the cache only (`WTD_CACHE_ONLY_URL_RETRIEVAL`), so the check never waits on
//! the network: an approval question must not hang on a CRL or OCSP server. A revoked certificate
//! is therefore not noticed here.
//!
//! A signature covers a file's content, never its name: a copy of cmd.exe saved as tool.exe (or as
//! notepad.exe) still verifies as Microsoft's. So the answer also has `original_name`, the
//! `OriginalFilename` of the executable's own (language-neutral) version resource, which is part of
//! what the signature covers. A signed file is known by its signer only when its file name is its
//! original name (`identity_key`, and the server's `identityKey`); the tiers look at both names.

use std::path::Path;

use serde_json::{json, Value};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, GENERIC_READ, HANDLE, HWND, INVALID_HANDLE_VALUE};
use windows::Win32::Security::Cryptography::Catalog::{
    CryptCATAdminAcquireContext2, CryptCATAdminCalcHashFromFileHandle2, CryptCATAdminEnumCatalogFromHash, CryptCATAdminReleaseCatalogContext,
    CryptCATAdminReleaseContext, CryptCATCatalogInfoFromContext, CATALOG_INFO,
};
use windows::Win32::Security::Cryptography::{CertGetNameStringW, CERT_NAME_RDN_TYPE, CERT_X500_NAME_STR};
use windows::Win32::Security::WinTrust::{
    WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_CATALOG_INFO, WINTRUST_DATA,
    WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_CATALOG, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE,
    WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetFileVersionInfoExW, GetFileVersionInfoSizeExW, VerQueryValueW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_VER_GET_NEUTRAL, OPEN_EXISTING,
};
use windows::Win32::Storage::Packaging::Appx::{GetPackageFamilyName, PackageFamilyNameFromFullName};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

use crate::{apps, windows_list};

/// `{ path, pid? }` → `{ signed, signer, file_name, original_name, package_family_name? }`. `pid`, when given, is
/// a process running `path` (a window's), whose package identity is read directly; otherwise a
/// package is recognized by its install folder. An executable that cannot be opened or verified is
/// `signed: false`, not an error: the server treats it as unsigned either way. `original_name` is
/// the version resource's `OriginalFilename` as `original_name_of` reads it, or null.
pub fn app_identity(params: &Value) -> Result<Value, (String, String)> {
    let invalid = |message: &str| ("invalid_params".to_string(), message.to_string());
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| is_absolute(path))
        .ok_or_else(|| invalid("path must be an absolute executable path."))?;
    let pid = match params.get("pid") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_u64().filter(|pid| *pid > 0).and_then(|pid| u32::try_from(pid).ok()).ok_or_else(|| invalid("pid must be a positive whole number."))?),
    };
    let file_name = Path::new(path).file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
    let family = pid.and_then(|pid| process_package_family(pid, path)).or_else(|| folder_package_family(path));
    if let Some(family) = family {
        return Ok(json!({ "signed": false, "signer": null, "file_name": file_name, "package_family_name": family }));
    }
    let signer = verified_signer(path);
    Ok(json!({ "signed": signer.is_some(), "signer": signer, "file_name": file_name, "original_name": original_name_of(path) }))
}

/// The `OriginalFilename` of `path`'s own version resource (`FILE_VER_GET_NEUTRAL`: never a `.mui`
/// file beside it, which anyone could put next to a copy), as `normalized_original_name` writes
/// it; None when the file has none or cannot be read. Each language the resource lists is tried,
/// then the usual ones.
fn original_name_of(path: &str) -> Option<String> {
    let wide = windows_list::utf16(path);
    unsafe {
        let mut handle = 0u32;
        let size = GetFileVersionInfoSizeExW(FILE_VER_GET_NEUTRAL, PCWSTR(wide.as_ptr()), &mut handle);
        if size == 0 {
            return None;
        }
        // u32 units, so the UTF-16 strings and the translation table inside are aligned.
        let mut data = vec![0u32; (size as usize).div_ceil(4)];
        GetFileVersionInfoExW(FILE_VER_GET_NEUTRAL, PCWSTR(wide.as_ptr()), None, size, data.as_mut_ptr().cast()).ok()?;
        let block = data.as_ptr() as *const core::ffi::c_void;
        let mut languages: Vec<(u16, u16)> = Vec::new();
        let mut table: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut length = 0u32;
        if VerQueryValueW(block, w!("\\VarFileInfo\\Translation"), &mut table, &mut length).as_bool() && !table.is_null() {
            let words = std::slice::from_raw_parts(table as *const u16, (length as usize / 4) * 2);
            languages.extend(words.chunks_exact(2).map(|pair| (pair[0], pair[1])));
        }
        languages.extend([(0x0409, 0x04B0), (0x0409, 0x04E4), (0x0000, 0x04B0)]);
        for (language, codepage) in languages {
            let query = windows_list::utf16(&format!("\\StringFileInfo\\{language:04x}{codepage:04x}\\OriginalFilename"));
            let mut value: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut chars = 0u32;
            if VerQueryValueW(block, PCWSTR(query.as_ptr()), &mut value, &mut chars).as_bool() && !value.is_null() && chars > 0 {
                let text = from_wide(std::slice::from_raw_parts(value as *const u16, chars as usize));
                if let Some(name) = normalized_original_name(&text) {
                    return Some(name);
                }
            }
        }
        None
    }
}

/// An `OriginalFilename` as the answer gives it: its last path part, lowercase, without the `.mui`
/// some Windows binaries add (`Cmd.Exe.MUI` is `cmd.exe`); None when nothing is left.
pub fn normalized_original_name(text: &str) -> Option<String> {
    let name = text.trim().rsplit(['\\', '/']).next().unwrap_or("").trim().to_lowercase();
    let name = name.strip_suffix(".mui").unwrap_or(&name).trim_end().to_string();
    (!name.is_empty()).then_some(name)
}

/// A file name without `.exe`, lowercase: how a file name and an original name are compared.
fn program_stem(name: &str) -> String {
    let lower = name.to_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

/// Whether the signed file `file_name` is the program its version resource names: only then is
/// it known by its signer (a renamed copy of another program is known by its path).
pub fn is_original_name(file_name: &str, original_name: Option<&str>) -> bool {
    original_name.is_some_and(|original| !original.is_empty() && program_stem(original) == program_stem(file_name))
}

/// A drive path (`C:\...`) or a UNC path (`\\server\...`).
fn is_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\') || path.starts_with(r"\\")
}

/// The package family of process `pid`, when it is packaged and runs `path`.
fn process_package_family(pid: u32, path: &str) -> Option<String> {
    let running = windows_list::process_path(pid)?;
    if !running.eq_ignore_ascii_case(path) {
        return None;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut length = 0u32;
        let sized = GetPackageFamilyName(handle, &mut length, None);
        let mut buffer = vec![0u16; length as usize];
        let read = sized == ERROR_INSUFFICIENT_BUFFER && length > 0 && GetPackageFamilyName(handle, &mut length, Some(PWSTR(buffer.as_mut_ptr()))) == ERROR_SUCCESS;
        let _ = CloseHandle(handle);
        read.then(|| from_wide(&buffer))
    }
}

/// The package family of an executable under `...\WindowsApps\<package full name>\`, when that
/// package is installed for this user in that very folder (a copy elsewhere is not the package).
fn folder_package_family(path: &str) -> Option<String> {
    // ASCII lower-casing keeps every byte offset, so `start` indexes `path` too (full Unicode
    // lower-casing changes the length of İ or the Kelvin sign); both sides are lowered the same way.
    let lower = path.to_ascii_lowercase();
    let start = lower.find(r"\windowsapps\")? + r"\windowsapps\".len();
    let full_name = path[start..].split('\\').next().filter(|name| !name.is_empty())?;
    let installed = apps::package_path(full_name)?;
    let folder = format!("{}\\", installed.trim_end_matches('\\').to_ascii_lowercase());
    if !lower.starts_with(&folder) {
        return None;
    }
    let wide = windows_list::utf16(full_name);
    unsafe {
        let mut length = 0u32;
        if PackageFamilyNameFromFullName(PCWSTR(wide.as_ptr()), &mut length, None) != ERROR_INSUFFICIENT_BUFFER || length == 0 {
            return None;
        }
        let mut buffer = vec![0u16; length as usize];
        if PackageFamilyNameFromFullName(PCWSTR(wide.as_ptr()), &mut length, Some(PWSTR(buffer.as_mut_ptr()))) != ERROR_SUCCESS {
            return None;
        }
        Some(from_wide(&buffer))
    }
}

/// The verified signer of `path`: its embedded signature, else a system catalog's; None when
/// neither verifies (or the file cannot be opened).
fn verified_signer(path: &str) -> Option<String> {
    let wide = windows_list::utf16(path);
    unsafe {
        let file = CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .ok()
        .filter(|handle| *handle != INVALID_HANDLE_VALUE)?;
        let mut info = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: PCWSTR(wide.as_ptr()),
            hFile: file,
            pgKnownSubject: std::ptr::null_mut(),
        };
        let mut data = trust_data();
        data.dwUnionChoice = WTD_CHOICE_FILE;
        data.Anonymous = WINTRUST_DATA_0 { pFile: &mut info };
        let signer = verify(&mut data).or_else(|| catalog_signer(file, &wide));
        let _ = CloseHandle(file);
        signer
    }
}

/// Looks the file's hash up in the system catalogs (SHA-256 ones first, then the older SHA-1
/// ones) and verifies the catalog that lists it.
unsafe fn catalog_signer(file: HANDLE, path: &[u16]) -> Option<String> {
    for algorithm in [w!("SHA256"), w!("SHA1")] {
        let mut admin = 0isize;
        if CryptCATAdminAcquireContext2(&mut admin, None, algorithm, None, None).is_err() {
            continue;
        }
        let signer = in_catalog(admin, file, path);
        let _ = CryptCATAdminReleaseContext(admin, 0);
        if signer.is_some() {
            return signer;
        }
    }
    None
}

unsafe fn in_catalog(admin: isize, file: HANDLE, path: &[u16]) -> Option<String> {
    let mut size = 0u32;
    let _ = CryptCATAdminCalcHashFromFileHandle2(admin, file, &mut size, None, None);
    if size == 0 {
        return None;
    }
    let mut hash = vec![0u8; size as usize];
    CryptCATAdminCalcHashFromFileHandle2(admin, file, &mut size, Some(hash.as_mut_ptr()), None).ok()?;
    let catalog = CryptCATAdminEnumCatalogFromHash(admin, &hash, None, None);
    if catalog == 0 {
        return None;
    }
    let mut info = CATALOG_INFO { cbStruct: std::mem::size_of::<CATALOG_INFO>() as u32, ..Default::default() };
    let signer = if CryptCATCatalogInfoFromContext(catalog, &mut info, 0).is_ok() {
        let tag = windows_list::utf16(&hash.iter().map(|byte| format!("{byte:02X}")).collect::<String>());
        let mut member = WINTRUST_CATALOG_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_CATALOG_INFO>() as u32,
            dwCatalogVersion: 0,
            pcwszCatalogFilePath: PCWSTR(info.wszCatalogFile.as_ptr()),
            pcwszMemberTag: PCWSTR(tag.as_ptr()),
            pcwszMemberFilePath: PCWSTR(path.as_ptr()),
            hMemberFile: file,
            pbCalculatedFileHash: hash.as_mut_ptr(),
            cbCalculatedFileHash: size,
            pcCatalogContext: std::ptr::null_mut(),
            hCatAdmin: admin,
        };
        let mut data = trust_data();
        data.dwUnionChoice = WTD_CHOICE_CATALOG;
        data.Anonymous = WINTRUST_DATA_0 { pCatalog: &mut member };
        verify(&mut data)
    } else {
        None
    };
    let _ = CryptCATAdminReleaseCatalogContext(admin, catalog, 0);
    signer
}

/// Trust data with no UI, no revocation check and no network retrieval (see the module comment).
fn trust_data() -> WINTRUST_DATA {
    WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_REVOCATION_CHECK_NONE | WTD_CACHE_ONLY_URL_RETRIEVAL,
        ..Default::default()
    }
}

/// Runs `WinVerifyTrust` on `data`; on success, the leaf certificate's subject. The state is
/// always closed again.
unsafe fn verify(data: &mut WINTRUST_DATA) -> Option<String> {
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let status = WinVerifyTrust(HWND(std::ptr::null_mut()), &mut action, data as *mut WINTRUST_DATA as *mut _);
    let signer = if status == 0 { leaf_subject(data.hWVTStateData) } else { None };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    let _ = WinVerifyTrust(HWND(std::ptr::null_mut()), &mut action, data as *mut WINTRUST_DATA as *mut _);
    signer
}

unsafe fn leaf_subject(state: HANDLE) -> Option<String> {
    let provider = WTHelperProvDataFromStateData(state);
    if provider.is_null() {
        return None;
    }
    let signer = WTHelperGetProvSignerFromChain(provider, 0, false, 0);
    if signer.is_null() || (*signer).csCertChain == 0 || (*signer).pasCertChain.is_null() {
        return None;
    }
    let leaf = (*(*signer).pasCertChain).pCert;
    if leaf.is_null() {
        return None;
    }
    let format = CERT_X500_NAME_STR.0;
    let para = Some(&format as *const u32 as *const core::ffi::c_void);
    let length = CertGetNameStringW(leaf, CERT_NAME_RDN_TYPE, 0, para, None);
    if length <= 1 {
        return None;
    }
    let mut buffer = vec![0u16; length as usize];
    CertGetNameStringW(leaf, CERT_NAME_RDN_TYPE, 0, para, Some(&mut buffer));
    Some(from_wide(&buffer)).filter(|subject| !subject.is_empty())
}

fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// Ticket 107 (ticket 118: moved here from the retired control panel): the key the server's
/// approval check gives an executable it knows only by its path
/// (`appKey`, src/policy.ts): `/` as `\`, lowercase. `to_lowercase` is the full Unicode mapping
/// (final sigma too), as JavaScript's `toLowerCase` is; an ASCII-only lowering would differ.
pub fn path_key(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

/// Ticket 107: the key the server's `identityKey` (src/identity.ts) gives the executable `path`
/// from the helper's `app_identity` answer `identity`: a package by its family (lowercase), a signed
/// file whose name is its original name (`is_original_name`) by its signer and lowercase file name,
/// anything else by its path (`path_key`). No answer, or one the server would not read
/// (`readAppIdentity`), is the path's key, as the server falls back.
pub fn identity_key(path: &str, identity: Option<&Value>) -> String {
    let text = |identity: &Value, name: &str| identity.get(name).and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_string);
    let readable = |identity: &&Value| {
        let signed = identity.get("signed").and_then(Value::as_bool);
        let family_ok = match identity.get("package_family_name") {
            None => true,
            Some(family) => family.as_str().is_some_and(|family| !family.is_empty()),
        };
        let original_ok = match identity.get("original_name") {
            None | Some(Value::Null) => true,
            Some(original) => original.as_str().is_some_and(|original| !original.is_empty()),
        };
        identity.is_object() && signed.is_some() && text(identity, "file_name").is_some() && (signed == Some(false) || text(identity, "signer").is_some()) && family_ok && original_ok
    };
    if let Some(identity) = identity.filter(readable) {
        if let Some(family) = text(identity, "package_family_name") {
            return format!("package:{}", family.to_lowercase());
        }
        if let (Some(true), Some(signer), Some(file)) = (identity.get("signed").and_then(Value::as_bool), text(identity, "signer"), text(identity, "file_name")) {
            if is_original_name(&file, text(identity, "original_name").as_deref()) {
                return format!("signer:{signer}|{}", file.to_lowercase());
            }
        }
    }
    path_key(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ticket 107: `appKey` (src/policy.ts) is `replaceAll("/", "\\").toLowerCase()`; JavaScript
    /// lowers the full Unicode way (node 24 gives "d:\\ünïcode\\σας\\i̇x\\ǆ.exe" for the path below:
    /// final sigma, İ as i and U+0307, the title-case ǅ), as Rust's `to_lowercase` does.
    #[test]
    fn a_path_key_is_the_servers_app_key() {
        assert_eq!(path_key("C:/Program Files/Contoso/App.EXE"), "c:\\program files\\contoso\\app.exe");
        assert_eq!(path_key("\\\\Server\\Share\\Tool.exe"), "\\\\server\\share\\tool.exe");
        assert_eq!(path_key("D:/Ünïcode/ΣΑΣ/İx/ǅ.EXE"), "d:\\ünïcode\\σας\\i\u{307}x\\ǆ.exe");
    }

    /// Ticket 107: `identityKey` (src/identity.ts) over what `readAppIdentity` accepts.
    #[test]
    fn an_identity_key_is_the_servers_identity_key() {
        let path = "C:/Apps/Demo.EXE";
        let key = |identity: Value| identity_key(path, Some(&identity));
        assert_eq!(key(json!({ "signed": false, "signer": null, "file_name": "demo.exe", "package_family_name": "Contoso.Demo_abc" })), "package:contoso.demo_abc");
        assert_eq!(
            key(json!({ "signed": true, "signer": "C=US, O=Contoso, CN=Contoso", "file_name": "Demo.EXE", "original_name": "demo.exe" })),
            "signer:C=US, O=Contoso, CN=Contoso|demo.exe",
            "the signer as it is, the file name lowercase"
        );
        assert_eq!(key(json!({ "signed": true, "signer": "CN=A", "file_name": "demo.exe", "original_name": "demo" })), "signer:CN=A|demo.exe", "compared without .exe");
        // A signed file that is not the program its version resource names (a renamed copy), or that names none, is known by its path.
        assert_eq!(key(json!({ "signed": true, "signer": "CN=A", "file_name": "demo.exe", "original_name": "cmd.exe" })), "c:\\apps\\demo.exe");
        assert_eq!(key(json!({ "signed": true, "signer": "CN=A", "file_name": "demo.exe", "original_name": null })), "c:\\apps\\demo.exe");
        assert_eq!(key(json!({ "signed": true, "signer": "CN=A", "file_name": "demo.exe" })), "c:\\apps\\demo.exe");
        assert_eq!(key(json!({ "signed": true, "signer": "CN=A", "file_name": "demo.exe", "original_name": "" })), "c:\\apps\\demo.exe");
        assert_eq!(key(json!({ "signed": false, "signer": null, "file_name": "demo.exe" })), "c:\\apps\\demo.exe", "unsigned: the path");
        assert_eq!(identity_key(path, None), "c:\\apps\\demo.exe", "no identity: the path, as the server falls back");
        // An answer the server would not read is no identity.
        assert_eq!(key(json!({ "signed": true, "signer": "", "file_name": "demo.exe" })), "c:\\apps\\demo.exe");
        assert_eq!(key(json!({ "signed": true, "file_name": "demo.exe" })), "c:\\apps\\demo.exe");
        assert_eq!(key(json!({ "signed": false, "file_name": "" })), "c:\\apps\\demo.exe");
        assert_eq!(key(json!({ "signed": "yes", "signer": "CN=A", "file_name": "demo.exe" })), "c:\\apps\\demo.exe");
        assert_eq!(key(json!({ "signed": false, "file_name": "demo.exe", "package_family_name": null })), "c:\\apps\\demo.exe");
        assert_eq!(key(json!({ "signed": true, "signer": "CN=A", "file_name": "demo.exe", "package_family_name": "" })), "c:\\apps\\demo.exe");
        assert_eq!(key(json!("signer:CN=A|demo.exe")), "c:\\apps\\demo.exe");
    }

    #[test]
    fn an_original_name_is_its_last_part_lowercase_without_mui() {
        assert_eq!(normalized_original_name("Cmd.Exe.MUI").as_deref(), Some("cmd.exe"));
        assert_eq!(normalized_original_name(" NOTEPAD.EXE ").as_deref(), Some("notepad.exe"));
        assert_eq!(normalized_original_name(r"C:\build\out\Tool.exe").as_deref(), Some("tool.exe"));
        assert_eq!(normalized_original_name("Code").as_deref(), Some("code"));
        for empty in ["", "   ", ".mui", r"C:\out\"] {
            assert_eq!(normalized_original_name(empty), None, "{empty:?}");
        }
        assert!(is_original_name("notepad.exe", Some("notepad.exe")));
        assert!(is_original_name("Code.exe", Some("code")));
        assert!(!is_original_name("notepad.exe", Some("cmd.exe")));
        assert!(!is_original_name("tool.exe", Some("cmd.exe")));
        assert!(!is_original_name("notepad.exe", None));
    }
}
