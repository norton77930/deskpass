//! `grant_mac` (ticket 94): the MACs of grants.json entries and of the file, under the helper's
//! grants key.
//!
//! The key is 32 random bytes (`BCryptGenRandom`, the system RNG) kept in `grants.key` beside
//! grants.json, in the `--grants-dir` folder the helper was started with (ticket 95: no request
//! names the file; ticket 96: the flag is required), protected with DPAPI for the current user (`CryptProtectData`, with a fixed
//! entropy naming its purpose, so no other DPAPI blob of the user's passes for it). Each request
//! unlocks it from the file (another helper process, such as another server's, may have made or
//! replaced it); it never leaves this process: no response, error or log line holds it, and its bytes are
//! zeroed when dropped.
//!
//! The server sends each entry as `{ kind, fields }`; this module checks that the fields are
//! exactly the kind's, of their types, and canonicalizes them: a JSON object with the names in
//! ascending order and no whitespace (serde_json's compact form: `"` and `\` escaped, control
//! characters as `\b \t \n \f \r` or `\u00xx`, everything else, U+2028 and characters outside the
//! BMP included, as UTF-8). One canonical form, here, for every writer of the file. A MAC is HMAC-SHA256 (CNG's `BCRYPT_HMAC_SHA256_ALG_HANDLE`) of that text, as 64
//! lower-case hex digits; the server compares them with what the file holds.
//!
//! - `grant`: `key`, `display_name`, `path`, `created_at`, `last_used_at`, all strings.
//! - `raised`: `key`, `tier`, `created_at`, all strings.
//! - `file`: `version` and `generation` (whole numbers), `grant_macs` and `raised_macs` (arrays
//!   of strings: the entries' MACs in file order).
//!
//! Without a key file, a new key is made (`created`). A file DPAPI refuses as damaged or as not
//! its own (`ERROR_INVALID_DATA`, `NTE_BAD_DATA`, or `NTE_BAD_KEY_STATE`: the master key is not
//! this profile's, as after a profile reset or on another machine), one that is not a DPAPI blob
//! (no DPAPI header), one that unlocks to anything but 32 bytes, and one over 4 KB (a key blob is a few hundred bytes) are replaced by a new key
//! (`lost_recreated`), and the server voids every entry. Any other failure (the file in use, no
//! permission, another DPAPI error) is `internal` and leaves the file alone, so a passing failure
//! never throws the key away. A new key is written to a temporary file and moved into place: a
//! first key only where there is none (a key another process made meanwhile is used instead), a
//! replacement over the old one.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{json, Map, Value};
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{LocalFree, ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, ERROR_INVALID_DATA, HLOCAL, NTE_BAD_DATA, NTE_BAD_KEY_STATE};
use windows::Win32::Security::Cryptography::{
    BCryptGenRandom, BCryptHash, CryptProtectData, CryptUnprotectData, BCRYPT_HMAC_SHA256_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG, CRYPTPROTECT_UI_FORBIDDEN,
    CRYPT_INTEGER_BLOB,
};
use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};

/// The key file's name, in the helper's grants folder (`key_path`).
pub const KEY_FILE_NAME: &str = "grants.key";
const KEY_LEN: usize = 32;
/// A key file larger than this is not a key blob: it is treated as damaged.
const KEY_FILE_MAX_BYTES: u64 = 4096;
/// DPAPI's optional entropy: a blob protected without it (or with other entropy) does not unlock.
const ENTROPY: &[u8] = b"deskpass grants.key v1";
/// The first bytes of every DPAPI blob: version 1 and the DPAPI provider's GUID.
const DPAPI_BLOB_HEADER: [u8; 20] = [1, 0, 0, 0, 0xd0, 0x8c, 0x9d, 0xdf, 0x01, 0x15, 0xd1, 0x11, 0x8c, 0x7a, 0x00, 0xc0, 0x4f, 0xc2, 0x97, 0xeb];

const GRANT_FIELDS: [&str; 5] = ["key", "display_name", "path", "created_at", "last_used_at"];
const RAISED_FIELDS: [&str; 3] = ["key", "tier", "created_at"];
const FILE_FIELDS: [&str; 4] = ["version", "generation", "grant_macs", "raised_macs"];

/// The grants key, zeroed when dropped.
struct Key([u8; KEY_LEN]);

impl Drop for Key {
    fn drop(&mut self) {
        for byte in self.0.iter_mut() {
            // A volatile write, so the zeroing is not optimized away as a dead store.
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
    }
}

/// Ticket 95: the folder `--grants-dir` named when the helper was started (ticket 96: the server
/// always passes it), read once.
static GRANTS_DIR_ARG: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Called once at start with the command line's `--grants-dir`, if any.
pub fn set_dir_arg(dir: Option<PathBuf>) {
    let _ = GRANTS_DIR_ARG.set(dir);
}

/// Ticket 96: the grants folder, the `--grants-dir` the helper was started with, or an error. One
/// folder resolver, in Node (`path.resolve(defaultLockDir())`, or the server's `grantsDir`): the
/// server always passes the flag. A helper started without it,
/// or with a folder that is not an absolute path, refuses every `grant_mac` (`internal`); it never
/// falls back to a folder of its own. No request can name another folder or file.
pub fn grants_dir() -> Result<PathBuf, (String, String)> {
    match GRANTS_DIR_ARG.get().cloned().flatten() {
        Some(dir) if dir.is_absolute() => Ok(dir),
        Some(_) => Err(internal("The grants folder the helper was started with (--grants-dir) is not an absolute path.")),
        None => Err(internal("The helper was started without a grants folder (--grants-dir).")),
    }
}

fn key_path() -> Result<PathBuf, (String, String)> {
    Ok(grants_dir()?.join(KEY_FILE_NAME))
}

/// `{ entries: [{ kind, fields }] }` → `{ macs: [hex], key_state: "ok" | "created" | "lost_recreated" }`.
/// Ticket 95: `key_path` is no longer a parameter (refused), the key is in the helper's own folder.
pub fn grant_mac(params: &Value) -> Result<Value, (String, String)> {
    let invalid = |message: &str| ("invalid_params".to_string(), message.to_string());
    if params.get("key_path").is_some() {
        return Err(invalid("key_path is not a parameter: the helper keeps the grants key in its own folder."));
    }
    let payloads = params
        .get("entries")
        .and_then(Value::as_array)
        .and_then(|entries| entries.iter().map(canonical_entry).collect::<Option<Vec<String>>>())
        .ok_or_else(|| invalid("entries must be an array of { kind: grant | raised | file, fields } with exactly that kind's fields."))?;
    let key_path = key_path()?;
    if let Some(dir) = key_path.parent() {
        std::fs::create_dir_all(dir).map_err(|error| internal(&format!("The grants folder could not be made: {error}")))?;
    }
    let (key, key_state) = open_key(&key_path)?;
    let macs = payloads.iter().map(|payload| hmac(&key, payload.as_bytes()).map(|mac| hex(&mac))).collect::<Result<Vec<String>, _>>()?;
    Ok(json!({ "macs": macs, "key_state": key_state }))
}

/// The canonical text of one `{ kind, fields }`, or None when it is not exactly one of the kinds.
fn canonical_entry(entry: &Value) -> Option<String> {
    let entry = entry.as_object()?;
    if entry.len() != 2 {
        return None;
    }
    let fields = entry.get("fields")?.as_object()?;
    let valid = match entry.get("kind")?.as_str()? {
        "grant" => exact(fields, &GRANT_FIELDS, |_, value| value.is_string()),
        "raised" => exact(fields, &RAISED_FIELDS, |_, value| value.is_string()),
        "file" => exact(fields, &FILE_FIELDS, |name, value| match name {
            "version" | "generation" => value.as_u64().is_some(),
            _ => value.as_array().is_some_and(|macs| macs.iter().all(Value::is_string)),
        }),
        _ => false,
    };
    if !valid {
        return None;
    }
    // Sorted by name here, whatever order the map keeps.
    let sorted: BTreeMap<&str, &Value> = fields.iter().map(|(name, value)| (name.as_str(), value)).collect();
    serde_json::to_string(&sorted).ok()
}

/// Whether `fields` has exactly `names`, each value of the type `typed` says.
fn exact(fields: &Map<String, Value>, names: &[&str], typed: impl Fn(&str, &Value) -> bool) -> bool {
    fields.len() == names.len() && names.iter().all(|name| fields.get(*name).is_some_and(|value| typed(name, value)))
}

fn internal(message: &str) -> (String, String) {
    ("internal".to_string(), message.to_string())
}

/// What became of reading the key file.
enum Unlocked {
    Key(Key),
    /// Damaged, not this profile's, or not a key: replaced by a new key.
    Lost,
    /// Any other failure: the file is left alone.
    Failed(String),
}

/// The key in `path`, unlocked, or a new one stored there, with the state the server is told.
fn open_key(path: &Path) -> Result<(Key, &'static str), (String, String)> {
    match read_key_file(path) {
        Ok(Some(blob)) => match blob.map_or(Unlocked::Lost, |blob| unprotect(&blob)) {
            Unlocked::Key(key) => Ok((key, "ok")),
            Unlocked::Lost => {
                let key = new_key()?;
                store(path, &key, true).map_err(|_| internal("A new grants key could not be written."))?;
                Ok((key, "lost_recreated"))
            }
            Unlocked::Failed(reason) => Err(internal(&format!("The grants key could not be unlocked ({reason})."))),
        },
        Ok(None) => {
            let key = new_key()?;
            match store(path, &key, false) {
                Ok(()) => Ok((key, "created")),
                // Another process made the first key meanwhile: that one is the key.
                Err(Stored::Exists) => match read_key_file(path) {
                    Ok(Some(Some(blob))) => match unprotect(&blob) {
                        Unlocked::Key(key) => Ok((key, "ok")),
                        _ => Err(internal("The grants key another process made could not be unlocked.")),
                    },
                    _ => Err(internal("The grants key could not be read.")),
                },
                Err(Stored::Failed) => Err(internal("A new grants key could not be written.")),
            }
        }
        Err(()) => Err(internal("The grants key could not be read.")),
    }
}

/// The key file's bytes: `Ok(None)` when there is none, `Ok(Some(None))` when it is too large to be a key blob.
fn read_key_file(path: &Path) -> Result<Option<Option<Vec<u8>>>, ()> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    let mut blob = Vec::new();
    file.take(KEY_FILE_MAX_BYTES + 1).read_to_end(&mut blob).map_err(|_| ())?;
    Ok(Some(if blob.len() as u64 > KEY_FILE_MAX_BYTES { None } else { Some(blob) }))
}

enum Stored {
    /// There was a key file already (only when not replacing).
    Exists,
    Failed,
}

/// Protects `key` and moves it into `path` through a temporary file beside it.
fn store(path: &Path, key: &Key, replace: bool) -> Result<(), Stored> {
    let blob = protect(key).ok_or(Stored::Failed)?;
    let folder = path.parent().ok_or(Stored::Failed)?;
    std::fs::create_dir_all(folder).map_err(|_| Stored::Failed)?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|time| time.as_nanos()).unwrap_or(0);
    let temp = folder.join(format!("{KEY_FILE_NAME}.{}.{nanos}.tmp", std::process::id()));
    std::fs::write(&temp, &blob).map_err(|_| Stored::Failed)?;
    let flags = if replace { MOVEFILE_WRITE_THROUGH | MOVEFILE_REPLACE_EXISTING } else { MOVEFILE_WRITE_THROUGH };
    let moved = unsafe { MoveFileExW(&HSTRING::from(temp.as_os_str()), &HSTRING::from(path.as_os_str()), flags) };
    if let Err(error) = moved {
        let _ = std::fs::remove_file(&temp);
        let exists = error.code() == ERROR_ALREADY_EXISTS.to_hresult() || error.code() == ERROR_FILE_EXISTS.to_hresult();
        return Err(if exists && !replace { Stored::Exists } else { Stored::Failed });
    }
    Ok(())
}

fn new_key() -> Result<Key, (String, String)> {
    let mut key = Key([0; KEY_LEN]);
    unsafe { BCryptGenRandom(None, &mut key.0, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }.ok().map_err(|_| internal("A grants key could not be generated."))?;
    Ok(key)
}

fn entropy() -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: ENTROPY.len() as u32, pbData: ENTROPY.as_ptr() as *mut u8 }
}

/// The DPAPI (CurrentUser) blob of `key`.
fn protect(key: &Key) -> Option<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB { cbData: KEY_LEN as u32, pbData: key.0.as_ptr() as *mut u8 };
    let entropy = entropy();
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(&input, w!("deskpass grants key"), Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output).ok()?;
        let blob = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        Some(blob)
    }
}

/// The key `blob` protects; `Lost` when DPAPI refuses it as damaged or not this profile's, or it is not 32 bytes.
fn unprotect(blob: &[u8]) -> Unlocked {
    // Not a DPAPI blob at all (DPAPI would call it an invalid parameter): not a key.
    if !blob.starts_with(&DPAPI_BLOB_HEADER) {
        return Unlocked::Lost;
    }
    let Ok(length) = u32::try_from(blob.len()) else { return Unlocked::Lost };
    let input = CRYPT_INTEGER_BLOB { cbData: length, pbData: blob.as_ptr() as *mut u8 };
    let entropy = entropy();
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        if let Err(error) = CryptUnprotectData(&input, None, Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output) {
            let code = error.code();
            return if code == ERROR_INVALID_DATA.to_hresult() || code == NTE_BAD_DATA || code == NTE_BAD_KEY_STATE {
                Unlocked::Lost
            } else {
                Unlocked::Failed(format!("{:#010x}", code.0 as u32))
            };
        }
        let mut unlocked = Unlocked::Lost;
        if output.cbData as usize == KEY_LEN {
            let mut key = Key([0; KEY_LEN]);
            key.0.copy_from_slice(std::slice::from_raw_parts(output.pbData, KEY_LEN));
            unlocked = Unlocked::Key(key);
        }
        // The unprotected bytes are zeroed before the buffer goes back to the heap.
        std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        unlocked
    }
}

fn hmac(key: &Key, payload: &[u8]) -> Result<[u8; 32], (String, String)> {
    let mut mac = [0u8; 32];
    unsafe { BCryptHash(BCRYPT_HMAC_SHA256_ALG_HANDLE, Some(&key.0), payload, &mut mac) }.ok().map_err(|_| internal("A grant MAC could not be computed."))?;
    Ok(mac)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
