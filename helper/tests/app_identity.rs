//! Ticket 93: `app_identity` tells the server who an app is: its Authenticode signer (the leaf
//! certificate's subject, verified with its chain) and file name, or a packaged app's package
//! family name. Not interactive: it sends no input and opens no window.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

use serde_json::{json, Value};

#[test]
fn a_catalog_signed_system_executable_reports_its_signer() {
    let mut helper = Helper::spawn();
    let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    // notepad.exe carries no embedded signature: it is signed through a system catalog.
    let notepad = format!(r"{system}\System32\notepad.exe");
    let identity = helper.request("app_identity", json!({ "path": notepad }));
    assert_eq!(identity["ok"], true, "{identity}");
    assert_eq!(identity["result"]["signed"], true, "{identity}");
    // The leaf certificate's full subject, in the order the certificate encodes it.
    assert_eq!(identity["result"]["signer"], "C=US, S=Washington, L=Redmond, O=Microsoft Corporation, CN=Microsoft Windows", "{identity}");
    assert_eq!(identity["result"]["file_name"], "notepad.exe", "{identity}");
    // Its own version resource names it, read without its .mui (which would say NOTEPAD.EXE.MUI).
    assert_eq!(identity["result"]["original_name"], "notepad.exe", "{identity}");
    assert!(identity["result"].get("package_family_name").is_none(), "{identity}");
}

#[test]
fn a_renamed_copy_of_a_signed_program_reports_the_program_it_is() {
    let mut helper = Helper::spawn();
    let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let folder = std::env::temp_dir().join(format!("deskpass-identity-{}", std::process::id()));
    std::fs::create_dir_all(&folder).expect("temp folder");
    // cmd.exe saved as notepad.exe: the signature still verifies (it covers the content, not the name).
    let copy = folder.join("notepad.exe");
    std::fs::copy(format!(r"{system}\System32\cmd.exe"), &copy).expect("copy cmd.exe");
    let identity = helper.request("app_identity", json!({ "path": copy.to_string_lossy() }));
    let _ = std::fs::remove_dir_all(&folder);
    assert_eq!(identity["ok"], true, "{identity}");
    assert_eq!(identity["result"]["signed"], true, "{identity}");
    assert_eq!(identity["result"]["file_name"], "notepad.exe", "{identity}");
    assert_eq!(identity["result"]["original_name"], "cmd.exe", "{identity}");
}

#[test]
fn an_embedded_signature_reports_the_same_signer_as_it_is_asked_again() {
    let mut helper = Helper::spawn();
    let edge = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe";
    if !std::path::Path::new(edge).is_file() {
        eprintln!("SKIP Edge is not installed at {edge}");
        return;
    }
    let first = helper.request("app_identity", json!({ "path": edge }));
    assert_eq!(first["result"]["signed"], true, "{first}");
    assert_eq!(first["result"]["signer"], "C=US, S=Washington, L=Redmond, O=Microsoft Corporation, CN=Microsoft Corporation", "{first}");
    assert_eq!(first["result"]["file_name"], "msedge.exe", "{first}");
    assert_eq!(first["result"]["original_name"], "msedge.exe", "{first}");
    let again = helper.request("app_identity", json!({ "path": edge.to_ascii_uppercase() }));
    assert_eq!(again["result"]["signer"], first["result"]["signer"], "{again}");
    assert_eq!(again["result"]["file_name"], "msedge.exe", "the file name is lower case: {again}");
}

#[test]
fn a_test_built_executable_is_unsigned() {
    let mut helper = Helper::spawn();
    let identity = helper.request("app_identity", json!({ "path": env!("CARGO_BIN_EXE_fixture-window") }));
    assert_eq!(identity["ok"], true, "{identity}");
    assert_eq!(identity["result"]["signed"], false, "{identity}");
    assert_eq!(identity["result"]["signer"], Value::Null, "{identity}");
    assert_eq!(identity["result"]["file_name"], "fixture-window.exe", "{identity}");
    assert!(identity["result"].get("package_family_name").is_none(), "{identity}");
    // A window's process that is not packaged: its own path is checked, as without a pid.
    let this = std::env::current_exe().expect("test exe").to_string_lossy().into_owned();
    let with_pid = helper.request("app_identity", json!({ "path": this, "pid": std::process::id() }));
    assert_eq!(with_pid["ok"], true, "{with_pid}");
    assert_eq!(with_pid["result"]["signed"], false, "{with_pid}");
    assert!(with_pid["result"].get("package_family_name").is_none(), "{with_pid}");
}

#[test]
fn a_missing_file_is_unsigned_and_a_bad_path_is_refused() {
    let mut helper = Helper::spawn();
    let missing = helper.request("app_identity", json!({ "path": r"C:\Contoso\NotThere\nothing93.exe" }));
    assert_eq!(missing["ok"], true, "{missing}");
    assert_eq!(missing["result"]["signed"], false, "{missing}");
    assert_eq!(missing["result"]["file_name"], "nothing93.exe", "{missing}");
    // Ticket 93 review: a path whose lower-casing changes its byte length (İ, the Kelvin sign) is answered, not a crash.
    for path in [r"C:\İ\WindowsApps\é\cmd.exe", "C:\\\u{212A}\\WindowsApps\\İx\\a.exe", r"C:\İİ\WINDOWSAPPS\Microsoft.WindowsCalculator_1_x64__8wekyb3d8bbwe\a.exe"] {
        let odd = helper.request("app_identity", json!({ "path": path }));
        assert_eq!(odd["ok"], true, "{path}: {odd}");
        assert_eq!(odd["result"]["signed"], false, "{path}: {odd}");
    }
    for params in [json!({}), json!({ "path": "" }), json!({ "path": "relative.exe" }), json!({ "path": 5 }), json!({ "path": r"C:\a.exe", "pid": "1" })] {
        let refused = helper.request("app_identity", params.clone());
        assert_eq!(refused["error"]["code"], "invalid_params", "{params}: {refused}");
    }
}

const CALCULATOR: &str = "uwp:Microsoft.WindowsCalculator_8wekyb3d8bbwe!App";

#[test]
fn a_uwp_app_reports_its_package_family_name() {
    let mut helper = Helper::spawn();
    let resolved = helper.request("resolve_app_id", json!({ "app_id": CALCULATOR }));
    if resolved["ok"] != true {
        eprintln!("SKIP Calculator is not installed: {resolved}");
        return;
    }
    let app = resolved["result"]["app"].as_str().expect("app").to_string();
    let identity = helper.request("app_identity", json!({ "path": app }));
    assert_eq!(identity["ok"], true, "{identity}");
    assert_eq!(identity["result"]["package_family_name"], "Microsoft.WindowsCalculator_8wekyb3d8bbwe", "{identity}");
    assert_eq!(identity["result"]["file_name"], "calculatorapp.exe", "{identity}");
    // The package's signature is Windows' to check (spec-phase4b): no Authenticode check is made.
    assert_eq!(identity["result"]["signed"], false, "{identity}");
    // The installed package's own full name under another WindowsApps folder is not the package.
    let start = app.to_ascii_lowercase().find(r"\windowsapps\").expect("a WindowsApps path") + r"\windowsapps\".len();
    let full_name = app[start..].split('\\').next().expect("full name");
    let elsewhere = std::env::temp_dir().join("WindowsApps").join(full_name).join("CalculatorApp.exe");
    let outside = helper.request("app_identity", json!({ "path": elsewhere.to_string_lossy() }));
    assert_eq!(outside["ok"], true, "{outside}");
    assert!(outside["result"].get("package_family_name").is_none(), "{outside}");
}

struct Helper {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl Helper {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_deskpass-helper"))
            .args(["--server-pid", &std::process::id().to_string()])
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
