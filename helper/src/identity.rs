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
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows::Win32::Storage::Packaging::Appx::{GetPackageFamilyName, PackageFamilyNameFromFullName};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

use crate::{apps, windows_list};

/// `{ path, pid? }` → `{ signed, signer, file_name, package_family_name? }`. `pid`, when given, is
/// a process running `path` (a window's), whose package identity is read directly; otherwise a
/// package is recognized by its install folder. An executable that cannot be opened or verified is
/// `signed: false`, not an error: the server treats it as unsigned either way.
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
    Ok(json!({ "signed": signer.is_some(), "signer": signer, "file_name": file_name }))
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
/// file by its signer and lowercase file name, anything else by its path (`path_key`). No answer,
/// or one the server would not read (`readAppIdentity`), is the path's key, as the server falls back.
pub fn identity_key(path: &str, identity: Option<&Value>) -> String {
    let text = |identity: &Value, name: &str| identity.get(name).and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_string);
    let readable = |identity: &&Value| {
        let signed = identity.get("signed").and_then(Value::as_bool);
        let family_ok = match identity.get("package_family_name") {
            None => true,
            Some(family) => family.as_str().is_some_and(|family| !family.is_empty()),
        };
        identity.is_object() && signed.is_some() && text(identity, "file_name").is_some() && (signed == Some(false) || text(identity, "signer").is_some()) && family_ok
    };
    if let Some(identity) = identity.filter(readable) {
        if let Some(family) = text(identity, "package_family_name") {
            return format!("package:{}", family.to_lowercase());
        }
        if let (Some(true), Some(signer), Some(file)) = (identity.get("signed").and_then(Value::as_bool), text(identity, "signer"), text(identity, "file_name")) {
            return format!("signer:{signer}|{}", file.to_lowercase());
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
        assert_eq!(key(json!({ "signed": true, "signer": "C=US, O=Contoso, CN=Contoso", "file_name": "Demo.EXE" })), "signer:C=US, O=Contoso, CN=Contoso|demo.exe", "the signer as it is, the file name lowercase");
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
}
