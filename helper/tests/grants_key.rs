//! Ticket 94: `grant_mac` canonicalizes grants.json entries (and the file's own fields) and signs
//! them with HMAC-SHA256 under a 32-byte random key the helper keeps in grants.key, protected with
//! DPAPI for the current user. The key never appears in a response; these tests see MACs and key
//! states, and one test unlocks the key itself to pin the canonical text (it never prints it).
//! Not interactive: no window, no input.

use std::io::{BufRead, BufReader, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde_json::{json, Value};
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{BCryptHash, CryptUnprotectData, BCRYPT_HMAC_SHA256_ALG_HANDLE, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};

/// The first bytes of every DPAPI blob: version 1 and the DPAPI provider's GUID.
const DPAPI_BLOB_HEADER: [u8; 20] = [1, 0, 0, 0, 0xd0, 0x8c, 0x9d, 0xdf, 0x01, 0x15, 0xd1, 0x11, 0x8c, 0x7a, 0x00, 0xc0, 0x4f, 0xc2, 0x97, 0xeb];

fn grant(key: &str, display_name: &str) -> Value {
    json!({ "kind": "grant", "fields": {
        "key": key, "display_name": display_name, "path": r"C:\Apps\Demo.exe",
        "created_at": "2026-01-01T00:00:00.000Z", "last_used_at": "2026-01-01T00:00:00.000Z" } })
}

// Test-only entries; the "aa" MACs are placeholders, not real signatures (the key is made fresh in a temp folder per test).
fn entries() -> Vec<Value> {
    vec![
        grant("signer:CN=Contoso|demo.exe", "Demo"),
        json!({ "kind": "raised", "fields": { "key": "signer:CN=Contoso|other.exe", "tier": "blocked", "created_at": "2026-01-02T00:00:00.000Z" } }),
        json!({ "kind": "file", "fields": { "version": 1, "generation": 7, "grant_macs": ["aa"], "raised_macs": [] } }),
    ]
}

#[test]
fn a_new_key_is_made_protected_and_gives_the_same_macs_when_opened_again() {
    let dir = TempDir::new("create");
    let key_path = dir.path().join("grants.key");
    let first = Helper::spawn(dir.path()).mac(&entries());
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(first["result"]["key_state"], "created", "{first}");
    let macs = macs_of(&first);
    assert_eq!(macs.len(), 3);
    assert_ne!(macs[0], macs[1], "different entries, different MACs");
    // Only the MACs and the key's state come back.
    let fields: Vec<&String> = first["result"].as_object().expect("result").keys().collect();
    assert_eq!(fields, ["key_state", "macs"], "{first}");
    // The file is a DPAPI blob, not the key itself.
    let blob = std::fs::read(&key_path).expect("grants.key");
    assert!(blob.len() > 32 && blob.starts_with(&DPAPI_BLOB_HEADER), "grants.key is a DPAPI blob ({} bytes)", blob.len());

    // Another helper process unlocks the same key: same MACs, and the file is left alone.
    let again = Helper::spawn(dir.path()).mac(&entries());
    assert_eq!(again["result"]["key_state"], "ok", "{again}");
    assert_eq!(macs_of(&again), macs);
    assert_eq!(std::fs::read(&key_path).expect("grants.key"), blob);
}

#[test]
fn the_canonical_text_is_pinned_down_to_its_escaping() {
    let dir = TempDir::new("canonical");
    let key_path = dir.path().join("grants.key");
    let mut helper = Helper::spawn(dir.path());
    let odd = "a\u{0001}b\u{2028}c\u{1F600}";
    let key = "signer:C=US, S=Washington, L=Redmond, O=Contoso Ltd, CN=Contoso Ltd|demo.exe";
    let answer = helper.mac(&[grant(key, odd), json!({ "kind": "file", "fields": { "version": 1, "generation": 7, "grant_macs": ["aa"], "raised_macs": [] } })]);
    let macs = macs_of(&answer);
    // The same text test/grants.test.ts pins for the server's side: \u0001 escaped, U+2028 and the emoji as UTF-8.
    let pinned_grant = "{\"created_at\":\"2026-01-01T00:00:00.000Z\",\"display_name\":\"a\\u0001b\u{2028}c\u{1F600}\",\"key\":\"signer:C=US, S=Washington, L=Redmond, O=Contoso Ltd, CN=Contoso Ltd|demo.exe\",\"last_used_at\":\"2026-01-01T00:00:00.000Z\",\"path\":\"C:\\\\Apps\\\\Demo.exe\"}";
    let pinned_file = r#"{"generation":7,"grant_macs":["aa"],"raised_macs":[],"version":1}"#;
    let expected = unlocked_hmacs(&std::fs::read(&key_path).expect("grants.key"), &[pinned_grant, pinned_file]);
    assert_eq!(macs, expected, "the helper's MACs are over the pinned texts");
}

#[test]
fn another_key_file_makes_the_old_macs_fail() {
    let first = TempDir::new("first");
    let second = TempDir::new("second");
    let mut helper = Helper::spawn(first.path());
    let old = macs_of(&helper.mac(&entries()));
    let other = macs_of(&Helper::spawn(second.path()).mac(&entries()));
    assert_ne!(old, other, "two new keys are two random keys");
    // The first folder's key file replaced by the second's: its MACs are the second key's now.
    std::fs::copy(second.path().join("grants.key"), first.path().join("grants.key")).expect("copy key");
    let replaced = helper.mac(&entries());
    assert_eq!(replaced["result"]["key_state"], "ok", "{replaced}");
    let now = macs_of(&replaced);
    for (now, old) in now.iter().zip(&old) {
        assert_ne!(now, old);
    }
    assert_eq!(now, other);
}

#[test]
fn a_key_file_that_cannot_be_unlocked_is_replaced_by_a_new_key() {
    let dir = TempDir::new("lost");
    let key_path = dir.path().join("grants.key");
    let mut helper = Helper::spawn(dir.path());
    let old = macs_of(&helper.mac(&entries()));
    // A damaged blob, which DPAPI refuses as bad data.
    let mut blob = std::fs::read(&key_path).expect("grants.key");
    let middle = blob.len() / 2;
    blob[middle] ^= 0xff;
    std::fs::write(&key_path, &blob).expect("corrupt key");
    let lost = helper.mac(&entries());
    assert_eq!(lost["ok"], true, "{lost}");
    assert_eq!(lost["result"]["key_state"], "lost_recreated", "{lost}");
    let renewed = macs_of(&lost);
    assert_ne!(renewed, old);
    // The new key is kept: asked again, it is unlocked and gives the same MACs.
    let kept = helper.mac(&entries());
    assert_eq!(kept["result"]["key_state"], "ok", "{kept}");
    assert_eq!(macs_of(&kept), renewed);
    // Something that is not a DPAPI blob at all, too, and a file too large to be a key blob.
    std::fs::write(&key_path, b"not a key").expect("garbage key");
    let garbage = helper.mac(&entries());
    assert_eq!(garbage["result"]["key_state"], "lost_recreated", "{garbage}");
    // A blob cut short.
    let whole = std::fs::read(&key_path).expect("grants.key");
    std::fs::write(&key_path, &whole[..whole.len() / 2]).expect("truncated key");
    let truncated = helper.mac(&entries());
    assert_eq!(truncated["result"]["key_state"], "lost_recreated", "{truncated}");
    std::fs::write(&key_path, vec![7u8; 5000]).expect("large key");
    assert_eq!(helper.mac(&entries())["result"]["key_state"], "lost_recreated");
    assert!(std::fs::metadata(&key_path).expect("grants.key").len() < 4096, "replaced by a key blob");
}

#[test]
fn a_key_file_that_cannot_be_read_is_an_error_and_is_left_alone() {
    let dir = TempDir::new("locked");
    let key_path = dir.path().join("grants.key");
    let mut helper = Helper::spawn(dir.path());
    macs_of(&helper.mac(&entries()));
    let blob = std::fs::read(&key_path).expect("grants.key");
    {
        // Held open with no sharing: the helper cannot read it.
        let _held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&key_path).expect("hold key");
        let refused = helper.mac(&entries());
        assert_eq!(refused["error"]["code"], "internal", "{refused}");
    }
    assert_eq!(std::fs::read(&key_path).expect("grants.key"), blob, "the key was not replaced");
    assert_eq!(helper.mac(&entries())["result"]["key_state"], "ok");
}

#[test]
fn bad_parameters_are_refused_and_no_file_is_written() {
    let dir = TempDir::new("params");
    let elsewhere = TempDir::new("elsewhere");
    let mut helper = Helper::spawn(dir.path());
    let grant_fields = grant("k", "Demo")["fields"].clone();
    let mut extra = grant_fields.clone();
    extra["mac"] = json!("aa");
    let mut numeric = grant_fields.clone();
    numeric["path"] = json!(5);
    for params in [
        json!({}),
        json!({ "entries": "a" }),
        json!({ "entries": ["a"] }),
        json!({ "entries": [{ "kind": "other", "fields": grant_fields }] }),
        json!({ "entries": [{ "kind": "grant", "fields": extra }] }),
        json!({ "entries": [{ "kind": "grant", "fields": numeric }] }),
        json!({ "entries": [{ "kind": "raised", "fields": grant_fields }] }),
        json!({ "entries": [{ "kind": "grant", "fields": grant_fields, "more": 1 }] }),
        json!({ "entries": [{ "kind": "file", "fields": { "version": 1, "generation": -1, "grant_macs": [], "raised_macs": [] } }] }),
        json!({ "entries": [{ "kind": "file", "fields": { "version": 1, "generation": 1.5, "grant_macs": [], "raised_macs": [] } }] }),
        json!({ "entries": [{ "kind": "file", "fields": { "version": 1, "generation": 1, "grant_macs": [1], "raised_macs": [] } }] }),
        // Ticket 95: no request names the key file; the folder is the one the helper was started with.
        json!({ "key_path": elsewhere.path().join("grants.key").to_string_lossy(), "entries": [] }),
    ] {
        let refused = helper.request("grant_mac", params.clone());
        assert_eq!(refused["error"]["code"], "invalid_params", "{params}: {refused}");
    }
    assert!(std::fs::read_dir(dir.path()).expect("dir").next().is_none(), "nothing was written");
    assert!(std::fs::read_dir(elsewhere.path()).expect("dir").next().is_none(), "nothing was written where key_path pointed");
    // No entries: nothing to sign, but the key is made (or unlocked) all the same, in the helper's folder.
    let empty = helper.request("grant_mac", json!({ "entries": [] }));
    assert_eq!(empty["result"]["macs"], json!([]), "{empty}");
    assert!(dir.path().join("grants.key").exists(), "the key is made in the --grants-dir folder");
}

/// Ticket 95: a `--grants-dir` that is not an absolute path is refused at every request, and
/// nothing is written, neither there nor in the user's own folder.
#[test]
fn a_grants_dir_that_is_not_absolute_is_refused() {
    let mut helper = Helper::spawn(Path::new("relative-grants"));
    let refused = helper.request("grant_mac", json!({ "entries": [] }));
    assert_eq!(refused["error"]["code"], "internal", "{refused}");
    assert!(!Path::new("relative-grants").exists(), "nothing was written");
    // `--grants-dir` last, with no folder after it: an error too, never the user's own folder.
    let mut helper = Helper::spawn_with(&[std::ffi::OsStr::new("--grants-dir")]);
    let refused = helper.request("grant_mac", json!({ "entries": [] }));
    assert_eq!(refused["error"]["code"], "internal", "{refused}");
}

/// Ticket 96: one folder resolver, in Node, which always passes `--grants-dir`. A helper started
/// without it refuses `grant_mac` and makes no key anywhere, `%LOCALAPPDATA%` included (pointed at
/// a temporary folder here, so the user's own folder is never touched even should this fail).
#[test]
fn a_helper_started_without_a_grants_dir_refuses_and_writes_nothing() {
    let local = TempDir::new("no-grants-dir");
    let mut child = Command::new(env!("CARGO_BIN_EXE_deskpass-helper"))
        .args(["--server-pid", &std::process::id().to_string()])
        .env("LOCALAPPDATA", local.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("helper");
    let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
    let mut line = String::new();
    reader.read_line(&mut line).expect("ready");
    writeln!(child.stdin.as_ref().unwrap(), "{}", json!({ "id": 1, "method": "grant_mac", "params": { "entries": [] } })).unwrap();
    let refused = loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let value: Value = serde_json::from_str(&line).unwrap_or_else(|_| json!({ "raw": line }));
        if value.get("event").is_none() {
            break value;
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(refused["error"]["code"], "internal", "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap_or_default().contains("--grants-dir"), "{refused}");
    assert_eq!(std::fs::read_dir(local.path()).expect("temp folder").count(), 0, "nothing was written under LOCALAPPDATA");
}

/// HMAC-SHA256 of each text under the key `blob` protects, unlocked here as the helper does; only the MACs leave.
fn unlocked_hmacs(blob: &[u8], texts: &[&str]) -> Vec<String> {
    const ENTROPY: &[u8] = b"deskpass grants.key v1";
    let input = CRYPT_INTEGER_BLOB { cbData: blob.len() as u32, pbData: blob.as_ptr() as *mut u8 };
    let entropy = CRYPT_INTEGER_BLOB { cbData: ENTROPY.len() as u32, pbData: ENTROPY.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&input, None, Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output).expect("unlock the key");
        let mut key = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        assert_eq!(key.len(), 32, "a 32-byte key");
        let macs = texts
            .iter()
            .map(|text| {
                let mut mac = [0u8; 32];
                BCryptHash(BCRYPT_HMAC_SHA256_ALG_HANDLE, Some(&key), text.as_bytes(), &mut mac).ok().expect("hmac");
                mac.iter().map(|byte| format!("{byte:02x}")).collect()
            })
            .collect();
        key.fill(0);
        macs
    }
}

fn macs_of(response: &Value) -> Vec<String> {
    let macs: Vec<String> = response["result"]["macs"].as_array().unwrap_or_else(|| panic!("macs: {response}")).iter().map(|mac| mac.as_str().expect("mac").to_string()).collect();
    for mac in &macs {
        assert!(mac.len() == 64 && mac.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)), "a MAC is 64 lower-case hex digits");
    }
    macs
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("cu-grants-{name}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Helper {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl Helper {
    /// A helper keeping its grants key in `grants_dir` (ticket 95's `--grants-dir`), never the user's folder.
    fn spawn(grants_dir: &Path) -> Self {
        Self::spawn_with(&[std::ffi::OsStr::new("--grants-dir"), grants_dir.as_os_str()])
    }

    /// A helper started with `extra` after `--server-pid`.
    fn spawn_with(extra: &[&std::ffi::OsStr]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_deskpass-helper"))
            .args(["--server-pid", &std::process::id().to_string()])
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("helper");
        let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
        let mut line = String::new();
        reader.read_line(&mut line).expect("ready");
        assert!(line.contains("ready"), "{line}");
        Self { child, reader, next_id: 1 }
    }

    fn mac(&mut self, entries: &[Value]) -> Value {
        self.request("grant_mac", json!({ "entries": entries }))
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(self.child.stdin.as_ref().unwrap(), "{}", json!({ "id": id, "method": method, "params": params })).unwrap();
        self.child.stdin.as_ref().unwrap().flush().unwrap();
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).unwrap();
            let value: Value = serde_json::from_str(&line).unwrap_or_else(|_| json!({ "raw": line }));
            if value.get("event").is_some() {
                continue;
            }
            return value;
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
