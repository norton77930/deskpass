//! UI Automation element trees (tickets 59, 60).
//!
//! Every UI Automation call runs on one dedicated worker thread. A call into another process
//! cannot be cancelled, so the caller waits for the worker at most `TREE_BUDGET`; past that it
//! answers with what the walk has reached (`truncated: timeout`) and leaves the worker behind. The
//! next request starts a fresh worker; the old one exits once its stuck call returns, without
//! reporting anything.
//!
//! The walk follows the Control View of the window and then of its own popups (the ones its
//! screenshot draws, `popups::target_popups`), skips what cannot be seen, and stays within the
//! depth, line and per-parent limits (`walk_tree`); a `query` (ticket 63) walks every child and
//! keeps only the matches and their ancestors (`walk_matching`). Each parent's children come in
//! one call with what describing them takes (ticket 69, `tree_request`), so the time is checked
//! between those calls and a call that hangs costs what that parent holds. A Chromium or Electron window
//! first gets up to 500 ms for its web content's tree (ticket 66, `wait_for_web_content`), once while
//! it stays empty under the same title (ticket 71, `EmptyWaits`). The helper only shapes the content (cut
//! names and values, states, actions); what to do with it is the server's. Element references of
//! each tree are kept (the newest `TREES_PER_WINDOW` trees per window) for the element actions:
//! a `click` on an element reads its position again when it clicks (ticket 62, `element_point`),
//! and `set_value` (64) and `element_action` (65) find the element again and act on it on the
//! worker (`act_on_element`).

use std::cell::{Cell, RefCell};
use std::collections::hash_map::RandomState;
use std::collections::VecDeque;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{
    AutomationElementMode_Full, CUIAutomation, ExpandCollapseState, ExpandCollapseState_Collapsed, ExpandCollapseState_Expanded, ExpandCollapseState_LeafNode,
    ExpandCollapseState_PartiallyExpanded, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement, IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern,
    IUIAutomationRangeValuePattern, IUIAutomationScrollItemPattern, IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern, IUIAutomationTreeWalker,
    IUIAutomationValuePattern, ToggleState, ToggleState_Indeterminate, ToggleState_Off, ToggleState_On, TreeScope, TreeScope_Children, TreeScope_Element,
    UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId, UIA_DocumentControlTypeId, UIA_ExpandCollapseExpandCollapseStatePropertyId, UIA_ExpandCollapsePatternId,
    UIA_HasKeyboardFocusPropertyId, UIA_InvokePatternId, UIA_IsEnabledPropertyId, UIA_IsKeyboardFocusablePropertyId, UIA_IsOffscreenPropertyId, UIA_IsPasswordPropertyId,
    UIA_NamePropertyId, UIA_RangeValuePatternId, UIA_RangeValueValuePropertyId, UIA_ScrollItemPatternId, UIA_SelectionItemIsSelectedPropertyId,
    UIA_SelectionItemPatternId, UIA_TogglePatternId, UIA_ToggleToggleStatePropertyId, UIA_ValueIsReadOnlyPropertyId, UIA_ValuePatternId, UIA_CONTROLTYPE_ID,
    UIA_PATTERN_ID, UIA_PROPERTY_ID,
};
use windows::Win32::UI::WindowsAndMessaging::{EnumChildWindows, GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, IsWindow};

use crate::capture;
use crate::input::{background_unavailable, BackgroundReason, Delivery};
use crate::esc;
use crate::idle;
use crate::popups;
use crate::protocol::Failure;
use crate::restore::{ForegroundWatch, Report};
use crate::windows_list::{self, check_expected_app, parse_hwnd, visible_frame};

/// How long a caller waits for a tree.
const TREE_BUDGET: Duration = Duration::from_secs(3);
/// Levels of the shown tree; children further down are not walked (`truncated: depth`).
const DEPTH_LIMIT: usize = 30;
/// Lines (numbered and text) after which the walk stops (`truncated: nodes`).
const NODE_LIMIT: usize = 500;
/// Children of one parent the walk visits; the rest are counted as omitted.
const CHILD_LIMIT: usize = 60;
/// Characters of a name the helper sends: one more than the server shows, so it still marks the cut.
const NAME_CHARS: usize = 81;
/// Characters of a value the helper sends (as many as the server shows); a longer value also
/// carries its full length (`value_length`), so a large document does not make the answer large.
const VALUE_CHARS: usize = 200;
/// Trees whose element references are kept per window; older ones are dropped.
const TREES_PER_WINDOW: usize = 5;

/// Control types whose elements are numbered (actionable) whatever patterns they support.
const INTERACTIVE: [i32; 16] = [
    50000, // button
    50031, // split button
    50004, // edit
    50030, // document
    50002, // check box
    50013, // radio button
    50003, // combo box
    50007, // list item
    50011, // menu item
    50019, // tab item
    50024, // tree item
    50005, // hyperlink
    50015, // slider
    50016, // spinner
    50029, // data item
    50035, // header item
];

/// Control types whose numeric value (RangeValue) is shown when they have no text value: slider,
/// spinner, scroll bar, progress bar and custom. The cache request brings RangeValue for every
/// element; this list only decides where it is shown.
const RANGED: [i32; 5] = [50015, 50016, 50014, 50012, 50025];

static NEXT_TREE: AtomicU64 = AtomicU64::new(1);

/// Five base-36 characters drawn once per helper process and put in each of its tree ids, so a
/// restarted helper does not hand out an id the previous one did (the counter starts over).
fn process_tag() -> &'static str {
    static TAG: OnceLock<String> = OnceLock::new();
    TAG.get_or_init(|| {
        // `RandomState` is seeded from the system's random source once per process.
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u32(std::process::id());
        hasher.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos()));
        let mut value = hasher.finish();
        (0..5)
            .map(|_| {
                let digit = (value % 36) as u32;
                value /= 36;
                char::from_digit(digit, 36).unwrap_or('0')
            })
            .collect()
    })
}

fn interrupted() -> Failure {
    ("interrupted".to_string(), esc::INTERRUPTED_MESSAGE.to_string()).into()
}

/// An element reference kept for later actions (ticket 62: `click` on an `element`). UI
/// Automation's client objects are free-threaded in the multithreaded apartment, which the main
/// thread joins for the helper's lifetime, so a reference stays valid after the worker that read
/// it ends. Its methods are called on a worker, and references are released there
/// (`release_on_worker`) so that a release, which may call into the app, does not hold up the
/// main thread; the main thread only moves references into and out of `TREES`. A debug build
/// checks that (m5): releasing one anywhere else panics. The STA threads (the approval dialog,
/// the overlay) never see one.
pub(crate) struct Element(IUIAutomationElement);
// SAFETY: see above; every thread that touches the reference is in the MTA.
unsafe impl Send for Element {}

impl Drop for Element {
    fn drop(&mut self) {
        debug_assert!(ON_WORKER.with(Cell::get), "a UI Automation element reference was released off the UI Automation worker");
    }
}

thread_local! {
    /// Set on the UI Automation workers (`run_worker`).
    static ON_WORKER: Cell<bool> = const { Cell::new(false) };
}

type Job = Box<dyn FnOnce(Result<&IUIAutomation, String>) + Send>;

/// The worker thread's queue; `None` until the first request, or after a worker was left behind.
static WORKER: Mutex<Option<Sender<Job>>> = Mutex::new(None);
static TREES: Mutex<TreeCache<Element>> = Mutex::new(TreeCache::new());

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs `job` on the worker thread, starting one if there is none.
fn submit(job: Job) {
    let mut worker = lock(&WORKER);
    let job = match worker.as_ref() {
        Some(jobs) => match jobs.send(job) {
            Ok(()) => return,
            // The worker is gone (it could not start); start another.
            Err(mpsc::SendError(job)) => job,
        },
        None => job,
    };
    let (jobs, queue) = mpsc::channel::<Job>();
    let _ = jobs.send(job);
    std::thread::spawn(move || run_worker(queue));
    *worker = Some(jobs);
}

/// Leaves the current worker behind: its queue is dropped, so it exits after its current job.
fn abandon_worker() {
    lock(&WORKER).take();
}

/// The worker: joins the MTA, runs jobs until its queue is dropped (it was left behind, or the
/// helper is ending), then releases what it holds and leaves COM again. The main thread stays in
/// the MTA, so references kept from this worker's trees outlive it.
fn run_worker(queue: Receiver<Job>) {
    ON_WORKER.with(|on| on.set(true));
    let joined = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let automation: Result<IUIAutomation, String> =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.map_err(|error| format!("UI Automation is not available: {error}"));
    for job in queue {
        job(automation.as_ref().map_err(Clone::clone));
    }
    drop(automation);
    if joined {
        unsafe { CoUninitialize() };
    }
}

/// Why `run_on_worker` has no answer.
enum WorkerError {
    /// The deadline passed. A call into another process cannot be cancelled, so the worker was
    /// left behind: it ends once that call returns, and its answer goes nowhere.
    TimedOut,
    /// UI Automation could not be started, or the worker ended without answering.
    Unavailable(String),
}

/// Runs `job` on the UI Automation worker and waits for its answer until `deadline` (tickets 59
/// and 62, and the element actions of 64 and 65). Past the deadline the worker is abandoned and
/// the next job starts a fresh one. What `job` returns crosses to the caller, so it must not hold
/// element references; what it captures is dropped on the worker.
fn run_on_worker<T: Send + 'static>(deadline: Instant, job: impl FnOnce(&IUIAutomation) -> T + Send + 'static) -> Result<T, WorkerError> {
    let (done, answer) = mpsc::channel();
    submit(Box::new(move |automation| {
        let _ = done.send(automation.map(job));
    }));
    match answer.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(message)) => Err(WorkerError::Unavailable(message)),
        Err(RecvTimeoutError::Timeout) => {
            abandon_worker();
            Err(WorkerError::TimedOut)
        }
        Err(RecvTimeoutError::Disconnected) => Err(WorkerError::Unavailable("the UI Automation thread ended unexpectedly".to_string())),
    }
}

/// Element references the main thread no longer needs: released on the worker, never here.
fn release_on_worker(elements: Vec<Element>) {
    if !elements.is_empty() {
        submit(Box::new(move |_| drop(elements)));
    }
}

/// What may still hold element references (a buffer a walk left behind fills): dropped on the worker.
fn drop_on_worker<T: Send + 'static>(value: T) {
    submit(Box::new(move |_| drop(value)));
}

/// The references of a tree being read, released on the worker unless they are kept.
struct Unkept(Vec<Element>);

impl Unkept {
    fn keep(mut self) -> Vec<Element> {
        std::mem::take(&mut self.0)
    }
}

impl Drop for Unkept {
    fn drop(&mut self) {
        release_on_worker(std::mem::take(&mut self.0));
    }
}

/// Who had a window handle: its process and thread. Windows reuses handles, and a handle that a
/// new window was given since a tree was read names another window, of another thread.
type Owner = (u32, u32);

fn owner_of(window: isize) -> Option<Owner> {
    let hwnd = HWND(window as *mut core::ffi::c_void);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return None;
        }
        let mut pid = 0u32;
        let thread = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        (thread != 0).then_some((pid, thread))
    }
}

/// What a walk has reached so far; the caller takes it when the walk ends or its time is up.
struct Walked<E> {
    elements: Vec<Value>,
    /// The numbered elements, index 1 first.
    refs: Vec<E>,
    focused_index: Option<usize>,
    /// Why the walk left elements out (`depth`, `nodes`); the caller adds `timeout`.
    truncated: Vec<&'static str>,
    /// Children left out where no shown element is above them (`(omitted N children)` at the top).
    omitted_children: usize,
    /// Ticket 66: a Chromium or Electron window whose web area still had no content when the wait
    /// for it ended (`wait_for_web_content`); false when it had some, was no such window, or the
    /// wait did not end.
    web_content_empty: bool,
}

impl<E> Default for Walked<E> {
    fn default() -> Self {
        Self { elements: Vec::new(), refs: Vec::new(), focused_index: None, truncated: Vec::new(), omitted_children: 0, web_content_empty: false }
    }
}

/// What a walk reads: the window, the frame its positions are relative to, its popups (handle and
/// visible frame), whether it is a Chromium or Electron window (`is_chromium`), and its title now
/// (what `EmptyWaits` remembers a wait with).
struct Scope {
    target: isize,
    frame: RECT,
    popups: Vec<(isize, RECT)>,
    chromium: bool,
    title: String,
}

enum Ended {
    Whole,
    OutOfTime,
    Stopped,
    Failed(String),
}

pub fn get_ui_tree(params: &Value) -> Result<Value, Failure> {
    let window_id = params.get("window_id").and_then(Value::as_str).unwrap_or("");
    let hwnd = parse_hwnd(window_id).ok_or_else(|| ("invalid_params".to_string(), "window_id is not a window handle.".to_string()))?;
    // Ticket 63: an optional text; empty is no query.
    let query = match params.get("query") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) if text.is_empty() => None,
        Some(Value::String(text)) => Some(fold(text)),
        Some(_) => return Err(("invalid_params".to_string(), "query must be text.".to_string()).into()),
    };
    // `get_window` refuses the helper's own windows (the approval dialog, the overlay) first.
    let info = windows_list::get_window(window_id)?;
    // Before the elevated check, as on the first version's paths: a window swapped to another
    // (elevated) app is `app_changed`.
    check_expected_app(hwnd, params)?;
    if info["elevated"] == true && info["helper_elevated"] != true {
        let name = info["app_name"].as_str().unwrap_or("This app");
        return Err((
            "elevated_target".to_string(),
            format!("{name} is running as administrator and cannot be operated. Report this to the user instead of retrying."),
        )
            .into());
    }
    let frame = visible_frame(hwnd).map_err(|message| ("window_not_found".to_string(), message))?;
    // The walk checks before each element, so a window without any would not see the stop.
    if esc::stopped() {
        return Err(interrupted());
    }
    // The target's own popups (menus, dropdowns, tooltips) by the rule the screenshot draws them
    // in with (ticket 38). Window handles and rects only: element objects stay on the worker.
    let popups: Vec<(isize, RECT)> = popups::target_popups(hwnd, frame).iter().map(|popup| (popup.hwnd.0 as isize, popup.frame)).collect();

    // Ticket 66: window classes only, like the popups' handles; the wait itself is on the worker.
    let scope = Scope { target: hwnd.0 as isize, frame, popups, chromium: is_chromium_window(hwnd), title: info["title"].as_str().unwrap_or("").to_string() };
    let deadline = Instant::now() + TREE_BUDGET;
    let walked = Arc::new(Mutex::new(Walked::default()));
    let shared = Arc::clone(&walked);
    let target = scope.target;
    let ended = match run_on_worker(deadline, move |automation| walk(automation, &scope, query.as_deref(), &shared, deadline)) {
        Ok(ended) => ended,
        Err(WorkerError::TimedOut) => Ended::OutOfTime,
        Err(WorkerError::Unavailable(message)) => Ended::Failed(message),
    };
    // A walk left behind may still add to its copy; this tree is what it had reached now.
    let Walked { elements, refs, focused_index, mut truncated, omitted_children, web_content_empty } = std::mem::take(&mut *lock(&walked));
    let refs = Unkept(refs);
    // That walk may add references until it ends, so whichever of it and this call lets go of the
    // buffer last does so on the worker.
    drop_on_worker(walked);
    let closed = || unsafe { !IsWindow(Some(hwnd)).as_bool() };
    match ended {
        // A walk that failed or ended while stopped is also cut short by the stop.
        Ended::Stopped => return Err(interrupted()),
        _ if esc::stopped() => return Err(interrupted()),
        Ended::Failed(_) if closed() => return Err(("window_not_found".to_string(), "The window is no longer open.".to_string()).into()),
        Ended::Failed(message) => return Err(("internal".to_string(), format!("The UI tree could not be read: {message}")).into()),
        Ended::Whole | Ended::OutOfTime if closed() => return Err(("window_not_found".to_string(), "The window is no longer open.".to_string()).into()),
        Ended::Whole | Ended::OutOfTime => {}
    }
    let Some(owner) = owner_of(target) else {
        return Err(("window_not_found".to_string(), "The window is no longer open.".to_string()).into());
    };
    let tree_id = format!("t{}-{}", NEXT_TREE.fetch_add(1, Ordering::SeqCst), process_tag());
    let dropped = lock(&TREES).insert(tree_id.clone(), target, owner, refs.keep(), owner_of);
    release_on_worker(dropped.into_iter().flatten().collect());
    let mut result = json!({
        "tree_id": tree_id,
        "window_title": info["title"],
        "elements": elements,
    });
    if let Some(index) = focused_index {
        result["focused_index"] = json!(index);
    }
    if matches!(ended, Ended::OutOfTime) {
        truncated.push("timeout");
    }
    if !truncated.is_empty() {
        result["truncated"] = json!(truncated);
    }
    if omitted_children > 0 {
        result["omitted_children"] = json!(omitted_children);
    }
    // Only ever `true` (the spec's protocol line): left out when the rule did not apply or was met.
    if web_content_empty {
        result["web_content_empty"] = json!(true);
    }
    Ok(result)
}

/// How long a caller waits for the UI Automation calls of an element action (spec-uia "hung apps").
const ACTION_BUDGET: Duration = Duration::from_secs(3);

/// Ticket 92: how long an input method waits for UI Automation to say whether the focused element
/// is a password field before it goes ahead as if it were not.
pub(crate) const PASSWORD_CHECK_LIMIT: Duration = Duration::from_secs(1);

/// Ticket 92: the refusal of content (text, a paste, a character key, a value, an element action)
/// for a password field, before any input was sent. `what` is the field: the focused element, or
/// the element acted on.
pub(crate) fn password_field(what: &str) -> Failure {
    Failure::from((
        "password_field".to_string(),
        format!("{what} is a password field, so no input was sent: Deskpass never enters passwords. Ask the user to type it themselves."),
    ))
    .with_input_sent(false)
}

/// Ticket 92: whether the element that has the keyboard focus says it is a password field
/// (UI Automation's IsPassword). False when it cannot tell: no focused element, IsPassword not
/// readable, or no answer by `deadline` (a hung app, no UI Automation), so input then goes ahead
/// as before; this is the guard's known limit.
pub(crate) fn focus_is_password(deadline: Instant) -> bool {
    run_on_worker(deadline, |automation| unsafe { automation.GetFocusedElement().ok().is_some_and(|element| is_password(&element)) }).unwrap_or(false)
}

/// IsPassword of `element`; false when it cannot be read (ticket 92 goes ahead then).
unsafe fn is_password(element: &IUIAutomationElement) -> bool {
    element.CurrentIsPassword().is_ok_and(|password| password.as_bool())
}

fn element_not_found() -> Failure {
    (
        "element_not_found".to_string(),
        "The element no longer exists or no longer belongs to the target window (it may have closed, or the window changed since its tree was read), so no input was sent. Read a new tree with computer_get_ui_tree and use a number from it; do not use another element instead.".to_string(),
    )
        .into()
}

/// An element of a kept tree, as an element action names it (m3): `click`'s `element`, or the
/// top-level `tree_id` and `index` of `set_value`.
pub(crate) struct ElementRef {
    tree_id: String,
    index: usize,
}

/// The `{tree_id, index}` of `params`; `prefix` is how the request names them in its errors
/// (`element.` for `click`).
pub(crate) fn element_ref(params: &Value, prefix: &str) -> Result<ElementRef, Failure> {
    let invalid = |message: String| -> Failure { ("invalid_params".to_string(), message).into() };
    let tree_id = params.get("tree_id").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or_else(|| invalid(format!("{prefix}tree_id is required.")))?.to_string();
    let index = params.get("index").and_then(Value::as_u64).ok_or_else(|| invalid(format!("{prefix}index must be a whole number from 1.")))?;
    let index = usize::try_from(index).map_err(|_| invalid(format!("{prefix}index is out of range.")))?;
    Ok(ElementRef { tree_id, index })
}

/// What an element action was waiting for when the app did not answer within `ACTION_BUDGET`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Waiting {
    /// Where to click: nothing was sent.
    Aim,
    /// The element and what it can do, before its action (ticket 65 review): the action was not
    /// done, and the job left behind will not do it (`Step::begin_action`).
    Prepare,
    /// The action itself, which UI Automation may still deliver to the app.
    Act,
    /// The element's state after an action the app took (ticket 65): the action is done.
    ReadBack,
}

/// `input_failed` for an element action whose UI Automation calls did not answer in time (M2):
/// the stuck worker was left behind, and nothing reports what it does later.
fn action_timeout(what: Waiting) -> Failure {
    let seconds = ACTION_BUDGET.as_secs();
    let message = match what {
        Waiting::Aim => format!(
            "The app did not answer the UI Automation request for the element's position within {seconds} seconds, so no input was sent. Take a screenshot before retrying."
        ),
        Waiting::Prepare => format!(
            "The app did not answer the UI Automation requests about the element within {seconds} seconds, so the action was not done (and will not be done later). Take a screenshot before retrying."
        ),
        Waiting::Act => format!(
            "The app did not respond to the UI Automation request within {seconds} seconds; it may still perform the action later. Take a screenshot before retrying."
        ),
        Waiting::ReadBack => format!(
            "The app took the action, but did not answer the UI Automation request for the element's new state within {seconds} seconds. Do not repeat the action: take a screenshot to check its result."
        ),
    };
    ("input_failed".to_string(), message).into()
}

/// The failure of an element action whose worker gave no answer.
fn worker_failure(error: WorkerError, what: Waiting) -> Failure {
    match error {
        WorkerError::TimedOut => action_timeout(what),
        WorkerError::Unavailable(message) => ("internal".to_string(), format!("The element could not be read: {message}")).into(),
    }
}

/// Ticket 62: the screen point a `click` on `element` (`{tree_id, index}`) goes to. The element's
/// position is read now, on the worker, not taken from when its tree was read; the point is the
/// centre of the part of it that is visible: in the target or one of its popups (`target_popups`,
/// as the screenshot and the click at a point count them) and on a monitor, and inside its
/// ancestors where it is (see `aim`).
pub(crate) fn element_point(hwnd: HWND, element: &Value) -> Result<(i32, i32), Failure> {
    let element = element_ref(element, "element.")?;
    let at = Placement::now(hwnd)?;
    let deadline = Instant::now() + ACTION_BUDGET;
    match run_on_worker(deadline, move |automation| locate(automation, &element, &at)) {
        Ok(point) => point,
        Err(error) => Err(worker_failure(error, Waiting::Aim)),
    }
}

/// Ticket 64: `set_value` replaces the text of element `{tree_id, index}` through its Value
/// pattern, with no keyboard input, after the target is made ready as for a click
/// (`prepare_target`). The value is read back afterwards (`read_back`, cut to as many characters
/// as a tree shows, with `read_back_length` when longer, and `read_back_matches`). A password field
/// is refused (ticket 92, `act_on_element`); one whose IsPassword cannot be read is written but,
/// as a password field might be, never read (`password: true`). The value written appears in no
/// answer, error or log line: a password field's must not, and no field's needs to.
pub fn set_value(params: &Value) -> Result<Value, Failure> {
    let value = params
        .get("value")
        .and_then(Value::as_str)
        .ok_or_else(|| -> Failure { ("invalid_params".to_string(), "value must be text.".to_string()).into() })?
        .to_string();
    // Ticket 139: delivered as `delivery_mode` says (foreground when absent), as `element_action` is.
    let delivery = Delivery::from_params(params)?;
    act_on_element(params, delivery, false, move |_, element, step| write_value(element, value, step))
}

/// Ticket 64 review: what the element actions (`set_value`, `element_action`) share. The window of
/// `params` is made ready as for a click (`prepare_target`: the foreground first), the element
/// `{tree_id, index}` is found again on the worker (`resolve`), and `job` acts on it there and
/// reads the result back in the same job, so no element reference crosses to this thread. `job`
/// answers what the result adds to `action_result`'s (`foreground_changed`). When the app does not
/// answer within `ACTION_BUDGET`, the timeout says what the worker was waiting for, as `job` marks
/// it on its `Step` (`Waiting::Prepare` until the action), and the job is marked abandoned, so a
/// job that has not reached its action yet never does it (`Step::begin_action`). A password field
/// is refused with `password_field` before `job` (ticket 92) unless `password_ok`: only the focus
/// action is (ticket 101), and the input tools still refuse content while the focus is there.
/// Ticket 138: `delivery` decides how the window is made ready (`Delivery::prepare`); in the
/// background nothing is raised, the foreground is watched from just before the action until
/// after it returned and given back when the target takes it (`ForegroundWatch`, ADR 0002), and
/// the result's `foreground_changed` compares the foreground with the one before the action
/// (`background_action_result`, with the restore's `foreground_restored` or `restore_failed`).
/// Ticket 149: in the background it first waits for the user to be idle (`idle::wait_for_idle`),
/// and the result says how long (`waited_for_idle_ms`, with `idle_wait_capped` at the cap).
fn act_on_element<J>(params: &Value, delivery: Delivery, password_ok: bool, job: J) -> Result<Value, Failure>
where
    J: FnOnce(&IUIAutomation, &IUIAutomationElement, &Step) -> Result<Value, Failure> + Send + 'static,
{
    let hwnd = crate::input::hwnd_from(params)?;
    let element = element_ref(params, "")?;
    act_on_found(hwnd, delivery, password_ok, Duration::ZERO, move |automation, at| unsafe { resolve(automation, &element, at) }.map(|resolved| resolved.element), job)
}

/// `act_on_element` for any way of finding the element: `find` gets it on the worker, where
/// `act_on_element` finds a kept one again (`resolve`). `finding` is the time `find` may take on
/// top of `ACTION_BUDGET` (ticket 141: the search of a point click, `TREE_BUDGET`), so that a long
/// search does not use up the action's.
fn act_on_found<F, J>(hwnd: HWND, delivery: Delivery, password_ok: bool, finding: Duration, find: F, job: J) -> Result<Value, Failure>
where
    F: FnOnce(&IUIAutomation, &Placement) -> Result<IUIAutomationElement, Failure> + Send + 'static,
    J: FnOnce(&IUIAutomation, &IUIAutomationElement, &Step) -> Result<Value, Failure> + Send + 'static,
{
    let _prepared = unsafe { delivery.prepare(hwnd) }?;
    // Ticket 149: once the checks passed, a background action waits for the user to stop typing
    // and release the mouse (at most `idle::WAIT_CAP`); a stop meanwhile does nothing. The user's
    // window is read after the wait: the user may have switched windows during it.
    let idle = match delivery {
        Delivery::Background => Some(idle::wait_for_idle().map_err(|idle::Stopped| interrupted())?),
        Delivery::Foreground => None,
    };
    let before = unsafe { GetForegroundWindow() };
    let at = Placement::now(hwnd)?;
    let deadline = Instant::now() + ACTION_BUDGET + finding;
    let step = Step::new(deadline);
    let reached = step.clone();
    let watch = match delivery {
        Delivery::Background => unsafe { ForegroundWatch::start(hwnd, before) },
        Delivery::Foreground => None,
    };
    // Set when the job ends, however it ends (a panic too): a watch kept past a timeout waits for it.
    let returned = Arc::new(AtomicBool::new(false));
    let job_returned = SetOnDrop(returned.clone());
    let done = run_on_worker(deadline, move |automation| {
        let _returned = job_returned;
        let element = find(automation, &at)?;
        // Ticket 92: nothing is done to a password field unless the action may be (ticket 101).
        if !password_ok && unsafe { is_password(&element) } {
            return Err(password_field("The element"));
        }
        job(automation, &element, &reached)
    });
    // Ticket 138: the foreground stays watched (and restored) until after the action returned,
    // whatever it answered. An action whose call was still running at the timeout may still be
    // taken when the app answers: its watch goes on until the call returns (`finish_when`).
    let late = keeps_watching(matches!(done, Err(WorkerError::TimedOut)), step.now());
    let restore = match watch {
        Some(watch) if late => {
            watch.finish_when(returned);
            Report::Unchanged
        }
        Some(watch) => watch.finish(),
        None => Report::Unchanged,
    };
    let done = match done {
        Ok(done) => done?,
        Err(_) if late => {
            step.abandon();
            return Err(late_action_timeout());
        }
        Err(error) => {
            step.abandon();
            return Err(worker_failure(error, step.now()));
        }
    };
    let mut result = match delivery {
        Delivery::Background => unsafe { crate::input::background_action_result(hwnd, before, restore) },
        Delivery::Foreground => unsafe { crate::input::action_result(hwnd) },
    };
    if let (Some(result), Value::Object(done)) = (result.as_object_mut(), done) {
        result.extend(done);
    }
    if let Some(idle) = idle {
        result["waited_for_idle_ms"] = json!(idle.ms);
        if idle.capped {
            result["idle_wait_capped"] = json!(true);
        }
    }
    Ok(result)
}

/// Whether a background action's foreground watch goes on past its request: only when the request
/// timed out while the action's own call was running (`Waiting::Act`). Before it, the job will not
/// act (`Step::begin_action`); after it, the action returned while it was watched.
fn keeps_watching(timed_out: bool, waiting: Waiting) -> bool {
    timed_out && waiting == Waiting::Act
}

/// The timeout of a background action whose call was still running (`keeps_watching`).
fn late_action_timeout() -> Failure {
    let seconds = ACTION_BUDGET.as_secs();
    let watch = crate::restore::LATE_ACTION_WATCH.as_secs();
    (
        "input_failed".to_string(),
        format!(
            "The app did not respond to the UI Automation request within {seconds} seconds; it may still perform the action later. Until it does (for up to {watch} seconds), Deskpass keeps giving the foreground back if the app takes it. Take a screenshot before retrying."
        ),
    )
        .into()
}

/// Sets its flag when dropped.
struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// An element action's job on the worker: what it has reached, for the caller's timeout message,
/// and whether the caller has given up on it.
#[derive(Clone)]
struct Step(Arc<StepState>);

struct StepState {
    waiting: Mutex<Waiting>,
    /// Set by the caller when it stopped waiting (review minor 1).
    abandoned: AtomicBool,
    /// When the caller stops waiting.
    deadline: Instant,
    /// The request the job belongs to (ticket 70), for `esc::begin_action`.
    request: Option<u64>,
}

impl Step {
    /// Made by the executing thread, for the request it is executing.
    fn new(deadline: Instant) -> Self {
        Step(Arc::new(StepState { waiting: Mutex::new(Waiting::Prepare), abandoned: AtomicBool::new(false), deadline, request: esc::current_request() }))
    }
    fn reached(&self, waiting: Waiting) {
        *lock(&self.0.waiting) = waiting;
    }
    fn now(&self) -> Waiting {
        *lock(&self.0.waiting)
    }
    fn deadline(&self) -> Instant {
        self.0.deadline
    }
    fn abandon(&self) {
        self.0.abandoned.store(true, Ordering::SeqCst);
    }
    /// Just before the action's own call: marks `Waiting::Act`, then refuses when the user stopped
    /// (`interrupted`), or the caller gave up on this job (abandoned, or its deadline is past, so
    /// its timeout says `Prepare` unless it read `Act` marked here first). A job that hung before
    /// its action therefore never does it once the app answers, and a retry cannot do it twice. A
    /// refusal for the deadline is the `Prepare` timeout, not a stop: an answer just past the
    /// deadline can still reach the caller, and no Esc was pressed. It is not asked after the
    /// action: a job that acted reads back as usual. Ticket 70: going ahead marks the request as
    /// acting (`esc::begin_action`, which checks the stop once more as it marks), so a stop from
    /// then on is answered with the text saying the action was taken.
    fn begin_action(&self) -> Result<(), Failure> {
        self.reached(Waiting::Act);
        if esc::stopped() {
            return Err(interrupted());
        }
        if self.0.abandoned.load(Ordering::SeqCst) || Instant::now() >= self.0.deadline {
            return Err(action_timeout(Waiting::Prepare));
        }
        if !esc::begin_action(self.0.request) {
            return Err(interrupted());
        }
        Ok(())
    }
}

/// The actions of `element_action` (ticket 65), named as a tree lists them, in its order.
#[derive(Clone, Copy, PartialEq)]
enum Action {
    Invoke,
    Toggle,
    Expand,
    Collapse,
    Select,
    ScrollIntoView,
    Focus,
}

const ACTIONS: [(Action, &str); 7] = [
    (Action::Invoke, "invoke"),
    (Action::Toggle, "toggle"),
    (Action::Expand, "expand"),
    (Action::Collapse, "collapse"),
    (Action::Select, "select"),
    (Action::ScrollIntoView, "scroll_into_view"),
    (Action::Focus, "focus"),
];

impl Action {
    fn name(self) -> &'static str {
        ACTIONS.iter().find(|(action, _)| *action == self).map_or("", |(_, name)| name)
    }
}

/// Ticket 65: `element_action` does `action` to element `{tree_id, index}` through its UI
/// Automation pattern (Invoke, Toggle, ExpandCollapse, SelectionItem, ScrollItem, or SetFocus),
/// after the target is made ready as for a click, and then reads back what there is to read:
/// `state` (toggle: `on`, `off` or `indeterminate`, with `previous_state` from before; expand and
/// collapse: `expanded`, `collapsed`, `partially expanded` or `leaf`; select: `selected` or `not
/// selected`; focus: `focused` or `not focused`). Invoke and scroll_into_view have none. What the
/// state should be is the server's to judge. A disabled element, and one without the action's
/// pattern, is `not_supported`, the latter with the actions it has (`data.supported_actions`).
/// On a password field only focus is done, so the user can type the password; every other action
/// is `password_field` (tickets 92, 101). Focus goes ahead too when IsPassword cannot be read.
pub fn element_action(params: &Value) -> Result<Value, Failure> {
    let named = params.get("action").and_then(Value::as_str);
    let action = ACTIONS.iter().find(|(_, name)| Some(*name) == named).map(|(action, _)| *action).ok_or_else(|| -> Failure {
        let names: Vec<&str> = ACTIONS.iter().map(|(_, name)| *name).collect();
        ("invalid_params".to_string(), format!("action must be one of {}.", names.join(", "))).into()
    })?;
    let delivery = Delivery::from_params(params)?;
    // Ticket 138: moving the keyboard focus brings the window forward, so it is never done in the
    // background; refused once the window is known to be the expected one, before anything is done.
    if delivery == Delivery::Background && action == Action::Focus {
        crate::input::hwnd_from(params)?;
        return Err(background_unavailable(BackgroundReason::FocusNeedsForeground));
    }
    act_on_element(params, delivery, action == Action::Focus, move |_, element, step| perform(element, action, step))
}

/// Ticket 140: a background left click without modifiers on element `{tree_id, index}`, done
/// through the first click pattern the element has (`click_actions`) exactly as `element_action`
/// does that action (a pattern the provider turns out not to have, `pattern_missing`, counts as
/// not had: the next is tried), so the target is not raised, the foreground is watched and given back, the
/// user's idle is waited for, and the result carries the same read-back (`state`,
/// `previous_state`) with `action`: the one it did. An element with none of the patterns is
/// `background_unavailable` (`no_click_pattern`) before anything is done. The clicks that need the
/// pointer are refused by the caller (`input::click`), before this.
pub fn click_element(params: &Value) -> Result<Value, Failure> {
    // `click` names the element as `element: {tree_id, index}`, an element action as the params' own.
    let reference = element_ref(params.get("element").unwrap_or(&Value::Null), "element.")?;
    let mut params = params.clone();
    params["tree_id"] = json!(reference.tree_id);
    params["index"] = json!(reference.index);
    act_on_element(&params, Delivery::Background, false, move |_, element, step| click_through_pattern(element, step))
}

/// Ticket 141: a background left click without modifiers at window pixel `(x, y)`, done on the
/// deepest element of the target's own UI tree that contains the point and has a click pattern
/// (`element_at_point`), exactly as `click_element` does on one it was given: the same pattern
/// order, foreground restore, idle wait and result. The point becomes a screen point as a click
/// at a point does (`input::screen_point`), but the screen is never asked what is there, so a
/// window over the target changes nothing and no other app is ever acted on. No such element is
/// `background_unavailable` (`no_element_at_point`) before anything is done. The search takes at
/// most `TREE_BUDGET`, as reading a tree does. The clicks that need the pointer are refused by the
/// caller (`input::click`), before this.
pub fn click_point(params: &Value, x: i32, y: i32) -> Result<Value, Failure> {
    let hwnd = crate::input::hwnd_from(params)?;
    act_on_found(
        hwnd,
        Delivery::Background,
        false,
        TREE_BUDGET,
        move |automation, at| {
            let popups: Vec<RECT> = at.popups.iter().map(|(_, rect)| *rect).collect();
            let point = crate::input::screen_point(at.frame, &popups, x, y)?;
            unsafe { element_at_point(automation, at, point, Instant::now() + TREE_BUDGET) }
        },
        move |_, element, step| click_through_pattern(element, step),
    )
}

/// On the worker: the click of `click_element`, through the first of the element's click patterns
/// (`click_actions`) that its provider really has; its answer, with `action`.
fn click_through_pattern(element: &IUIAutomationElement, step: &Step) -> Result<Value, Failure> {
    for action in click_actions(&unsafe { abilities_of(element) }) {
        let missing = Cell::new(false);
        match perform_marking(element, action, step, &missing) {
            Ok(mut done) => {
                done["action"] = json!(action.name());
                return Ok(done);
            }
            // The element reported the pattern but the provider has none (a Win32 list item
            // and Invoke): nothing was done, so the next pattern is tried.
            Err(_) if missing.get() => continue,
            Err(failure) => return Err(failure),
        }
    }
    Err(background_unavailable(BackgroundReason::NoClickPattern))
}

/// On the worker (ticket 141): the deepest element of the Control View below the target, or of one
/// of its popups (looked at first: they lie above it), that is on screen, contains the screen
/// `point` and has a click pattern (`click_actions`); of equal depth the first. Only elements that
/// contain the point are followed down, so an element is chosen only when every ancestor contains
/// it too (a scrolled-out part of a list is not hit through its clipped parent). One call into the
/// app for each element followed (`tree_request`, as a tree read does). Nothing found is
/// `background_unavailable` (`no_element_at_point`); a stop is `interrupted`, and a search not
/// finished by `deadline` `input_failed`: what is deeper there is not known, so none is picked.
unsafe fn element_at_point(automation: &IUIAutomation, at: &Placement, point: (i32, i32), deadline: Instant) -> Result<IUIAutomationElement, Failure> {
    let internal = |error: windows::core::Error| -> Failure { ("internal".to_string(), format!("UI Automation is not available: {error}")).into() };
    let children = tree_request(automation, TreeScope_Children).map_err(internal)?;
    let itself = tree_request(automation, TreeScope_Element).map_err(internal)?;
    let mut found = None;
    let mut roots = Vec::new();
    for (popup, frame) in &at.popups {
        if contains(frame, point) {
            if let Ok(element) = automation.ElementFromHandleBuildCache(HWND(*popup as *mut core::ffi::c_void), &itself) {
                roots.push((element, true));
            }
        }
    }
    let target = automation
        .ElementFromHandle(HWND(at.target as *mut core::ffi::c_void))
        .map_err(|_| -> Failure { ("window_not_found".to_string(), "The window is no longer open.".to_string()).into() })?;
    roots.push((target, false));
    for (root, is_popup) in roots {
        let mut best = None;
        if is_popup {
            consider(&root, 0, point, &mut best);
        }
        search_below(&UiaNode { element: root, children: &children }, 1, point, deadline, &mut best)?;
        found = best.map(|(_, element)| element);
        if found.is_some() {
            break;
        }
    }
    found.ok_or_else(|| background_unavailable(BackgroundReason::NoElementAtPoint))
}

/// Whether the screen `point` is in `rect`.
fn contains(rect: &RECT, point: (i32, i32)) -> bool {
    point.0 >= rect.left && point.1 >= rect.top && point.0 < rect.right && point.1 < rect.bottom
}

/// `element` as a candidate of `element_at_point` at `depth`: it is when it is on screen, contains
/// `point` and has a click pattern, and no candidate of the same or less depth is held in `best`.
/// Answers whether it contains the point (and is on screen), that is, whether to look below it.
unsafe fn consider(element: &IUIAutomationElement, depth: usize, point: (i32, i32), best: &mut Option<(usize, IUIAutomationElement)>) -> bool {
    let cached = Cached::of(element);
    if cached.offscreen || !cached.rect.is_some_and(|rect| contains(&rect, point)) {
        return false;
    }
    let abilities = Abilities {
        invoke: cached.invoke,
        toggle: cached.toggle.is_some(),
        expand: cached.expand,
        select: cached.selected.is_some(),
        scroll_item: cached.scroll_item,
        focusable: cached.focusable,
    };
    if !click_actions(&abilities).is_empty() && best.as_ref().is_none_or(|(held, _)| depth > *held) {
        *best = Some((depth, element.clone()));
    }
    true
}

/// `element_at_point` below `parent`: each child that contains the point is considered, and its
/// own children searched in turn. Asked before each fetch of children whether the user stopped or
/// the time is up.
unsafe fn search_below(parent: &UiaNode, depth: usize, point: (i32, i32), deadline: Instant, best: &mut Option<(usize, IUIAutomationElement)>) -> Result<(), Failure> {
    if esc::stopped() {
        return Err(interrupted());
    }
    // Levels past the one a parent chain is followed to (`PARENT_LIMIT`) are not looked at.
    if depth > PARENT_LIMIT {
        return Ok(());
    }
    if Instant::now() >= deadline {
        return Err((
            "input_failed".to_string(),
            format!(
                "The app did not finish listing the elements at that point within {} seconds, so no input was sent. Take a screenshot before retrying.",
                TREE_BUDGET.as_secs()
            ),
        )
            .into());
    }
    for child in parent.children() {
        if consider(&child.element, depth, point, best) {
            search_below(&child, depth + 1, point, deadline, best)?;
        }
    }
    Ok(())
}

/// The element actions a click can be, in the order of spec-background: Invoke, then Toggle, then
/// SelectionItem.Select, then ExpandCollapse by the element's state (expand unless it is
/// expanded, which collapses it); those the element has. The first is the one used. Empty when it
/// has none of them.
fn click_actions(abilities: &Abilities) -> Vec<Action> {
    let mut actions = Vec::new();
    if abilities.invoke {
        actions.push(Action::Invoke);
    }
    if abilities.toggle {
        actions.push(Action::Toggle);
    }
    if abilities.select {
        actions.push(Action::Select);
    }
    if let Some(state) = abilities.expand {
        actions.push(if state == ExpandCollapseState_Expanded { Action::Collapse } else { Action::Expand });
    }
    actions
}

/// `element_action` on the worker. A stop, and a caller that gave up, are checked just before the
/// action (`Step::begin_action`); after it only a stop is, before each read back (`settle`).
fn perform(element: &IUIAutomationElement, action: Action, step: &Step) -> Result<Value, Failure> {
    perform_marking(element, action, step, &Cell::new(false))
}

/// UI Automation's answer when a provider does not have the pattern it was asked to act through:
/// UIA_E_NOTSUPPORTED, or the managed client-side provider's InvalidOperationException ("Pattern
/// not found", COR_E_INVALIDOPERATION), which a Win32 list item answers to Invoke although the
/// element reported the pattern.
fn pattern_missing(error: &windows::core::Error) -> bool {
    const UIA_E_NOTSUPPORTED: u32 = 0x8004_0204;
    const COR_E_INVALIDOPERATION: u32 = 0x8013_1509;
    matches!(error.code().0 as u32, UIA_E_NOTSUPPORTED | COR_E_INVALIDOPERATION)
}

/// `perform`, setting `missing` when the failure was the provider not having the pattern (nothing
/// was done), for `click_element` to go on to the next pattern.
fn perform_marking(element: &IUIAutomationElement, action: Action, step: &Step, missing: &Cell<bool>) -> Result<Value, Failure> {
    let not_supported = |message: String| -> Failure { ("not_supported".to_string(), message).into() };
    let gone_or = |error: windows::core::Error, otherwise: Failure| if element_gone(&error) { element_not_found() } else { otherwise };
    let name = action.name();
    unsafe {
        // Fail closed, as set_value does: an element whose state cannot be read is not acted on.
        let enabled = element
            .CurrentIsEnabled()
            .map_err(|error| gone_or(error, not_supported(format!("UI Automation could not tell whether this element is enabled, so {name} was not done."))))?;
        if !enabled.as_bool() {
            return Err(not_supported(format!("This element is disabled, so {name} was not done.")));
        }
        let unsupported = |error: Option<windows::core::Error>| match error {
            Some(error) if element_gone(&error) => element_not_found(),
            _ => unsupported_action(element, action),
        };
        let failed = |error: windows::core::Error| {
            missing.set(pattern_missing(&error));
            action_failure(&error, name)
        };
        let mut read = json!({});
        match action {
            Action::Invoke => {
                let pattern: IUIAutomationInvokePattern = element.GetCurrentPatternAs(UIA_InvokePatternId).map_err(|error| unsupported(Some(error)))?;
                step.begin_action()?;
                pattern.Invoke().map_err(failed)?;
            }
            Action::Toggle => {
                let pattern: IUIAutomationTogglePattern = element.GetCurrentPatternAs(UIA_TogglePatternId).map_err(|error| unsupported(Some(error)))?;
                let before = pattern.CurrentToggleState().ok().map(toggle_name);
                if let Some(before) = before {
                    read["previous_state"] = json!(before);
                }
                step.begin_action()?;
                pattern.Toggle().map_err(failed)?;
                step.reached(Waiting::ReadBack);
                let after = read_back(|| pattern.CurrentToggleState().ok().map(toggle_name), |state| Some(state) != before, READ_BACK_WAIT, step.deadline())?;
                if let Some(state) = after {
                    read["state"] = json!(state);
                }
            }
            Action::Expand | Action::Collapse => {
                let pattern: IUIAutomationExpandCollapsePattern = element.GetCurrentPatternAs(UIA_ExpandCollapsePatternId).map_err(|error| unsupported(Some(error)))?;
                // A leaf has nothing to expand, as the tree has it.
                if pattern.CurrentExpandCollapseState().is_ok_and(|state| state == ExpandCollapseState_LeafNode) {
                    return Err(unsupported(None));
                }
                step.begin_action()?;
                if action == Action::Expand { pattern.Expand() } else { pattern.Collapse() }.map_err(failed)?;
                step.reached(Waiting::ReadBack);
                let expected = if action == Action::Expand { "expanded" } else { "collapsed" };
                if let Some(state) = read_back(|| pattern.CurrentExpandCollapseState().ok().map(expand_name), |state| state == expected, READ_BACK_WAIT, step.deadline())? {
                    read["state"] = json!(state);
                }
            }
            Action::Select => {
                let pattern: IUIAutomationSelectionItemPattern = element.GetCurrentPatternAs(UIA_SelectionItemPatternId).map_err(|error| unsupported(Some(error)))?;
                step.begin_action()?;
                pattern.Select().map_err(failed)?;
                step.reached(Waiting::ReadBack);
                let selected = || pattern.CurrentIsSelected().ok().map(|selected| if selected.as_bool() { "selected" } else { "not selected" });
                if let Some(state) = read_back(selected,|state| state == "selected", READ_BACK_WAIT, step.deadline())? {
                    read["state"] = json!(state);
                }
            }
            Action::ScrollIntoView => {
                let pattern: IUIAutomationScrollItemPattern = element.GetCurrentPatternAs(UIA_ScrollItemPatternId).map_err(|error| unsupported(Some(error)))?;
                step.begin_action()?;
                pattern.ScrollIntoView().map_err(failed)?;
            }
            Action::Focus => {
                // As the tree offers focus: only to an element that says it can take the focus.
                match element.CurrentIsKeyboardFocusable() {
                    Ok(focusable) if focusable.as_bool() => {}
                    Ok(_) => return Err(unsupported(None)),
                    Err(error) => return Err(unsupported(Some(error))),
                }
                step.begin_action()?;
                element.SetFocus().map_err(failed)?;
                step.reached(Waiting::ReadBack);
                let focused = || element.CurrentHasKeyboardFocus().ok().map(|focused| if focused.as_bool() { "focused" } else { "not focused" });
                if let Some(state) = read_back(focused,|state| state == "focused", READ_BACK_WAIT, step.deadline())? {
                    read["state"] = json!(state);
                }
            }
        }
        Ok(read)
    }
}

/// How long a read-back waits for the state an action should leave. Some providers act after their
/// call returns (a Win32 check box's Toggle posts its click), so the first read can be too early.
const READ_BACK_WAIT: Duration = Duration::from_millis(300);

/// How often a read-back reads again while it waits.
const READ_BACK_INTERVAL: Duration = Duration::from_millis(30);

/// How long before its job's deadline a read-back stops waiting, so that its answer, rather than
/// the timeout, reaches the caller.
const READ_BACK_MARGIN: Duration = Duration::from_millis(250);

/// A stop was seen while `settle` waited.
struct Stopped;

/// What `read` gives once `done` holds of it, or what it gives when the wait ends (None: it could
/// not be read): read every `interval` until `wait` is over, or `margin` before the job's
/// `deadline` if that is sooner; it reads once at least, and never sleeps past the end of the
/// wait. A stop is checked before each read. The one loop for waiting on another app: the
/// read-backs of the element actions (`read_back`, ticket 65) and the Chromium web content
/// (`wait_for_web_content`, ticket 66) differ in what they read and how long they wait.
fn settle<T>(read: impl Fn() -> Option<T>, done: impl Fn(&T) -> bool, wait: Duration, interval: Duration, margin: Duration, deadline: Instant) -> Result<Option<T>, Stopped> {
    let now = Instant::now();
    let until = (now + wait).min(deadline.checked_sub(margin).unwrap_or(now));
    loop {
        if esc::stopped() {
            return Err(Stopped);
        }
        let state = read();
        if state.as_ref().is_some_and(&done) || Instant::now() >= until {
            return Ok(state);
        }
        std::thread::sleep(interval.min(until.saturating_duration_since(Instant::now())));
    }
}

/// `settle` for the state an element action should leave: a stop is `stopped_after_action`, the
/// action having been taken.
fn read_back(read: impl Fn() -> Option<&'static str>, done: impl Fn(&str) -> bool, wait: Duration, deadline: Instant) -> Result<Option<&'static str>, Failure> {
    settle(read, |state| done(state), wait, READ_BACK_INTERVAL, READ_BACK_MARGIN, deadline).map_err(|Stopped| stopped_after_action())
}

/// Ticket 66: how long a tree read waits for a Chromium or Electron window's web content, and how
/// often it looks (spec-uia "Chromium / Electron"). The time counts inside `TREE_BUDGET`.
const WEB_WAIT: Duration = Duration::from_millis(500);
const WEB_INTERVAL: Duration = Duration::from_millis(50);
/// How long before the tree's deadline the wait stops at the latest, so that the walk has time:
/// half the budget. The wait starts as the job does, so this only matters for a job that started
/// late; the 500 ms above is what ends it otherwise.
const WEB_MARGIN: Duration = Duration::from_millis(1500);

/// Waits for `has_content` to hold, as `WEB_WAIT` allows; true when it still did not (the web
/// content is empty), false as soon as it did.
fn wait_for_web_content(has_content: impl Fn() -> bool, deadline: Instant) -> Result<bool, Stopped> {
    settle(|| Some(has_content()), |content| *content, WEB_WAIT, WEB_INTERVAL, WEB_MARGIN, deadline).map(|content| content != Some(true))
}

/// Ticket 71: the Chromium windows (handle) whose web content was still empty when a tree read
/// had waited for it, each with the window's title then, oldest first. A window found empty is not
/// waited for again while its title stays: Chromium changes the title with the page, and VS Code
/// with the file. Handles and text only, no element objects, so nothing here needs the worker.
struct EmptyWaits {
    windows: VecDeque<(isize, String)>,
}

/// Windows `EmptyWaits` remembers at most; past that the oldest are forgotten.
const EMPTY_WAITS_LIMIT: usize = 32;

static EMPTY_WAITS: Mutex<EmptyWaits> = Mutex::new(EmptyWaits::new());

impl EmptyWaits {
    const fn new() -> Self {
        Self { windows: VecDeque::new() }
    }

    /// Whether `window` was found empty while it had this `title`.
    fn knows(&self, window: isize, title: &str) -> bool {
        self.windows.iter().any(|(known, known_title)| *known == window && known_title == title)
    }

    /// Notes that `window` was found empty with this `title`, instead of what it was noted with
    /// (one title per window). Windows that `is_open` says are gone are dropped now, and the oldest
    /// ones when there are more than `EMPTY_WAITS_LIMIT`.
    fn remember(&mut self, window: isize, title: &str, is_open: impl Fn(isize) -> bool) {
        self.windows.retain(|(known, _)| *known != window && is_open(*known));
        self.windows.push_back((window, title.to_string()));
        while self.windows.len() > EMPTY_WAITS_LIMIT {
            self.windows.pop_front();
        }
    }

    /// Forgets `window`: its web content has some.
    fn forget(&mut self, window: isize) {
        self.windows.retain(|(known, _)| *known != window);
    }
}

fn window_is_open(window: isize) -> bool {
    unsafe { IsWindow(Some(HWND(window as *mut core::ffi::c_void))).as_bool() }
}

/// Ticket 71: the web content is ready when the Document with the largest area has children
/// (`(area, has children)` of each Document found), so a side bar that has some is not enough. Of
/// Documents of the same area it is enough that one has; with none found it is not ready. A
/// Document whose rectangle cannot be read has area 0 (`area_of`), so it only counts when no
/// other Document is bigger.
fn main_content_ready(documents: &[(i64, bool)]) -> bool {
    let Some(largest) = documents.iter().map(|(area, _)| *area).max() else { return false };
    documents.iter().any(|(area, has_children)| *area == largest && *has_children)
}

/// The area of a bounding rectangle in pixels; 0 when it could not be read or is empty.
fn area_of(rect: Option<RECT>) -> i64 {
    rect.map_or(0, |rect| i64::from((rect.right - rect.left).max(0)) * i64::from((rect.bottom - rect.top).max(0)))
}

/// `interrupted` for a stop between an action and its read-back (review minor 3): the action was
/// taken, so it is not to be done again. The same text as the notifier's for a stop after
/// `Step::begin_action` (ticket 70).
fn stopped_after_action() -> Failure {
    ("interrupted".to_string(), esc::STOPPED_AFTER_ACTION_MESSAGE.to_string()).into()
}

fn toggle_name(state: windows::Win32::UI::Accessibility::ToggleState) -> &'static str {
    match state {
        state if state == ToggleState_On => "on",
        state if state == ToggleState_Indeterminate => "indeterminate",
        _ => "off",
    }
}

fn expand_name(state: windows::Win32::UI::Accessibility::ExpandCollapseState) -> &'static str {
    match state {
        state if state == ExpandCollapseState_Expanded => "expanded",
        state if state == ExpandCollapseState_PartiallyExpanded => "partially expanded",
        state if state == ExpandCollapseState_LeafNode => "leaf",
        _ => "collapsed",
    }
}

/// `not_supported` for `action` on an element without it, listing the element actions it has
/// (`data.supported_actions`, in the tree's order) in the data and the message.
fn unsupported_action(element: &IUIAutomationElement, action: Action) -> Failure {
    let supported = unsafe { supported_actions(element) };
    let has = if supported.is_empty() { "none of computer_element_action's actions".to_string() } else { supported.join(", ") };
    Failure {
        code: "not_supported".to_string(),
        message: format!("This element does not support {}; it supports {has}.", action.name()),
        data: Some(json!({ "supported_actions": supported })),
    }
}

/// What an element can do through `element_action`, as the tree (`describe`) and `element_action`
/// (`supported_actions`) both read it: one pattern for each action, the keyboard focus for focus.
struct Abilities {
    invoke: bool,
    toggle: bool,
    /// The ExpandCollapse state; None without the pattern, or for a leaf (nothing to expand).
    expand: Option<windows::Win32::UI::Accessibility::ExpandCollapseState>,
    select: bool,
    scroll_item: bool,
    focusable: bool,
}

impl Abilities {
    /// Whether the element has `action`. `by_state`: as a tree lists them, expand only when it is not
    /// expanded and collapse only when it is not collapsed (a partly expanded element has both);
    /// otherwise, as `element_action` takes them, both whatever its state is now.
    fn allows(&self, action: Action, by_state: bool) -> bool {
        match action {
            Action::Invoke => self.invoke,
            Action::Toggle => self.toggle,
            Action::Expand => self.expand.is_some_and(|state| !by_state || state != ExpandCollapseState_Expanded),
            Action::Collapse => self.expand.is_some_and(|state| !by_state || state != ExpandCollapseState_Collapsed),
            Action::Select => self.select,
            Action::ScrollIntoView => self.scroll_item,
            Action::Focus => self.focusable,
        }
    }

    /// The names of the actions it has (`allows`), in `ACTIONS`' order.
    fn actions(&self, by_state: bool) -> Vec<&'static str> {
        ACTIONS.iter().filter(|(action, _)| self.allows(*action, by_state)).map(|(_, name)| *name).collect()
    }
}

/// The element actions `element` has, as `element_action` decides them (`Abilities`, not by state).
unsafe fn supported_actions(element: &IUIAutomationElement) -> Vec<&'static str> {
    abilities_of(element).actions(false)
}

/// What `element` has of the element actions (`Abilities`), read from its patterns now.
unsafe fn abilities_of(element: &IUIAutomationElement) -> Abilities {
    let has = |pattern| element.GetCurrentPattern(pattern).is_ok();
    Abilities {
        invoke: has(UIA_InvokePatternId),
        toggle: has(UIA_TogglePatternId),
        expand: element
            .GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId)
            .ok()
            .and_then(|pattern| pattern.CurrentExpandCollapseState().ok())
            .filter(|state| *state != ExpandCollapseState_LeafNode),
        select: has(UIA_SelectionItemPatternId),
        scroll_item: has(UIA_ScrollItemPatternId),
        focusable: element.CurrentIsKeyboardFocusable().is_ok_and(|flag| flag.as_bool()),
    }
}

/// The failure of the pattern call of an element action.
fn action_failure(error: &windows::core::Error, name: &str) -> Failure {
    const UIA_E_ELEMENTNOTENABLED: u32 = 0x8004_0200;
    if error.code().0 as u32 == UIA_E_ELEMENTNOTENABLED {
        return ("not_supported".to_string(), format!("This element is disabled, so {name} was not done.")).into();
    }
    if element_gone(error) {
        return element_not_found();
    }
    ("input_failed".to_string(), format!("UI Automation could not do {name} on the element ({}). Take a screenshot before retrying.", error.message())).into()
}

/// `set_value` on the worker. Like `perform`, it checks a stop and a caller that gave up just
/// before SetValue (`Step::begin_action`).
fn write_value(element: &IUIAutomationElement, value: String, step: &Step) -> Result<Value, Failure> {
    let not_supported = |message: &str| -> Failure { ("not_supported".to_string(), message.to_string()).into() };
    let gone_or = |error: windows::core::Error, otherwise: Failure| if element_gone(&error) { element_not_found() } else { otherwise };
    unsafe {
        let pattern: IUIAutomationValuePattern = element.GetCurrentPatternAs(UIA_ValuePatternId).map_err(|error| {
            gone_or(error, not_supported("This element does not accept a value directly. Click it and use computer_type_text instead."))
        })?;
        let read_only = "This element is read-only, so its value cannot be set.";
        // Fail closed, as the tree does: a field whose state cannot be read is not written, and
        // one whose IsPassword cannot be read is treated as a password field.
        if pattern.CurrentIsReadOnly().map_err(|error| gone_or(error, not_supported(read_only)))?.as_bool() {
            return Err(not_supported(read_only));
        }
        // Review minor 2: a disabled element is refused before SetValue, whatever its provider
        // would do with it (a disabled Win32 edit's proxy refuses, but leaves that edit reported as
        // the foreground window, so the next input is refused). Fail closed as for IsReadOnly: an
        // element whose state cannot be read is not written.
        let enabled = element.CurrentIsEnabled().map_err(|error| gone_or(error, not_supported("UI Automation could not tell whether this element is enabled, so its value was not set.")))?;
        if !enabled.as_bool() {
            return Err(not_supported(DISABLED));
        }
        let password = element.CurrentIsPassword().map(|flag| flag.as_bool()).unwrap_or(true);
        step.begin_action()?;
        pattern.SetValue(&windows::core::BSTR::from(value.as_str())).map_err(|error| set_value_failure(&error, password))?;
        if password {
            return Ok(json!({ "password": true }));
        }
        let Ok(now) = pattern.CurrentValue() else { return Ok(json!({})) };
        let now = now.to_string();
        let matches = now == value;
        let (shown, length) = cut_text(now, VALUE_CHARS);
        let mut read = json!({ "read_back": shown, "read_back_matches": matches });
        if let Some(length) = length {
            read["read_back_length"] = json!(length);
        }
        Ok(read)
    }
}

const DISABLED: &str = "This element is disabled, so its value cannot be set.";

/// Ticket 77 (`paste_text`): the element that has the keyboard focus in the app of `input` (the
/// target's `keyboard_window`), by its RuntimeId, and its value, when that element has a Value
/// pattern and is not a password field (nor one that cannot tell); None otherwise, and when UI
/// Automation does not answer by `deadline`. The value is only compared (`focused_value_shows`),
/// never shown or logged.
pub(crate) fn focused_value(input: HWND, deadline: Instant) -> Option<Focused> {
    let input = input.0 as isize;
    run_on_worker(deadline, move |automation| unsafe { read_focused_value(automation, input) }).ok().flatten()
}

/// What `focused_value` read: the focused element (its RuntimeId) and its value.
pub(crate) struct Focused {
    runtime_id: Vec<i32>,
    value: String,
}

/// Ticket 77: waits up to `wait` until the focused element is still the one of `before` and its
/// value (read as `focused_value` reads it) differs from `before`'s and contains `text`, line breaks
/// counted alike whether CR LF, CR or LF. False when it did not by then (another element that took
/// the focus does not count, whatever it shows), when input was stopped meanwhile, and when UI
/// Automation did not answer in time.
pub(crate) fn focused_value_shows(input: HWND, before: Focused, text: &str, wait: Duration) -> bool {
    let input = input.0 as isize;
    let (element, before, text) = (before.runtime_id, line_feeds(&before.value), line_feeds(text));
    let deadline = Instant::now() + wait + READ_BACK_MARGIN;
    let shows = move |now: &Focused| now.runtime_id == element && now.value != before && now.value.contains(text.as_str());
    let found = run_on_worker(deadline, move |automation| {
        let read = || unsafe { read_focused_value(automation, input) }.map(|now| Focused { value: line_feeds(&now.value), ..now });
        settle(read, &shows, wait, READ_BACK_INTERVAL, READ_BACK_MARGIN, deadline).ok().flatten().is_some_and(|now| shows(&now))
    });
    found.unwrap_or(false)
}

unsafe fn read_focused_value(automation: &IUIAutomation, input: isize) -> Option<Focused> {
    let mut pid = 0u32;
    GetWindowThreadProcessId(HWND(input as *mut core::ffi::c_void), Some(&mut pid));
    let element = automation.GetFocusedElement().ok()?;
    if element.CurrentProcessId().ok()? as u32 != pid || element.CurrentIsPassword().map_or(true, |password| password.as_bool()) {
        return None;
    }
    let pattern: IUIAutomationValuePattern = element.GetCurrentPatternAs(UIA_ValuePatternId).ok()?;
    let value = pattern.CurrentValue().ok()?.to_string();
    Some(Focused { runtime_id: runtime_id(&element)?, value })
}

/// The element's RuntimeId (a one-dimensional array of 32-bit integers), which tells it from every
/// other element while it exists.
unsafe fn runtime_id(element: &IUIAutomationElement) -> Option<Vec<i32>> {
    let array = element.GetRuntimeId().ok()?;
    if array.is_null() {
        return None;
    }
    let id = {
        let header = &*array;
        (header.cDims == 1 && header.cbElements == 4 && !header.pvData.is_null())
            .then(|| std::slice::from_raw_parts(header.pvData as *const i32, header.rgsabound[0].cElements as usize).to_vec())
    };
    let _ = SafeArrayDestroy(array);
    id
}

// oleaut32's SafeArrayDestroy, linked directly: the windows crate has it behind a feature this
// crate does not otherwise need.
#[link(name = "oleaut32")]
extern "system" {
    fn SafeArrayDestroy(array: *const windows::Win32::System::Com::SAFEARRAY) -> windows::core::HRESULT;
}

/// `text` with CR LF and lone CR as LF.
fn line_feeds(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// The failure of a SetValue call. The error's text can carry the provider's own description
/// (IErrorInfo), which a password field's provider may fill with anything, the value included, so
/// for a password field (or one whose IsPassword could not be read) the sentence is fixed.
fn set_value_failure(error: &windows::core::Error, password: bool) -> Failure {
    const UIA_E_ELEMENTNOTENABLED: u32 = 0x8004_0200;
    if error.code().0 as u32 == UIA_E_ELEMENTNOTENABLED {
        return ("not_supported".to_string(), DISABLED.to_string()).into();
    }
    if element_gone(error) {
        return element_not_found();
    }
    let message = if password {
        "UI Automation could not set the value of the password field. Take a screenshot before retrying.".to_string()
    } else {
        format!("UI Automation could not set the value ({}). Take a screenshot before retrying.", error.message())
    };
    ("input_failed".to_string(), message).into()
}

/// Where the target is now (M2), read on the calling thread for the worker: window handles and
/// rectangles only, as element objects stay on the worker.
pub(crate) struct Placement {
    target: isize,
    /// Who has the target's handle now (None: closed).
    owner: Option<Owner>,
    frame: RECT,
    /// The target's popups (`target_popups`): handle and visible frame.
    popups: Vec<(isize, RECT)>,
    monitors: Vec<RECT>,
}

impl Placement {
    pub(crate) fn now(hwnd: HWND) -> Result<Self, Failure> {
        let frame = visible_frame(hwnd).map_err(|message| ("window_not_found".to_string(), message))?;
        let popups = popups::target_popups(hwnd, frame).iter().map(|popup| (popup.hwnd.0 as isize, popup.frame)).collect();
        let target = hwnd.0 as isize;
        Ok(Self { target, owner: owner_of(target), frame, popups, monitors: capture::monitor_rects() })
    }
}

/// An element found again for an action, on the worker.
struct Resolved {
    element: IUIAutomationElement,
    /// Its Control View ancestors below the target or the popup it is in, nearest first.
    ancestors: Vec<IUIAutomationElement>,
}

/// On the worker (M2): `element` of its kept tree, still in the target or one of its popups. The
/// reference is cloned with the cache locked and used after, so a call into a hung app never holds
/// the cache. An element that is gone, or no longer below the target or a popup, is
/// `element_not_found`.
unsafe fn resolve(automation: &IUIAutomation, element: &ElementRef, at: &Placement) -> Result<Resolved, Failure> {
    let found = lock(&TREES).find(&element.tree_id, at.target, at.owner, element.index)?.0.clone();
    let walker = automation.ControlViewWalker().map_err(|error| -> Failure { ("internal".to_string(), format!("UI Automation is not available: {error}")).into() })?;
    let target = automation
        .ElementFromHandle(HWND(at.target as *mut core::ffi::c_void))
        .map_err(|_| -> Failure { ("window_not_found".to_string(), "The window is no longer open.".to_string()).into() })?;
    let mut roots = vec![target];
    roots.extend(at.popups.iter().filter_map(|(popup, _)| automation.ElementFromHandle(HWND(*popup as *mut core::ffi::c_void)).ok()));
    let ancestors = ancestors_below(automation, &walker, &found, &roots).ok_or_else(element_not_found)?;
    Ok(Resolved { element: found, ancestors })
}

/// `element_point` on the worker.
fn locate(automation: &IUIAutomation, element: &ElementRef, at: &Placement) -> Result<(i32, i32), Failure> {
    unsafe {
        let Resolved { element, ancestors } = resolve(automation, element, at)?;
        let rect = element.CurrentBoundingRectangle().map_err(|error| {
            if element_gone(&error) {
                return element_not_found();
            }
            ("input_failed".to_string(), format!("UI Automation could not read the element's position ({}), so no input was sent. Take a screenshot before retrying.", error.message()))
                .into()
        })?;
        let clips: Vec<RECT> = ancestors.iter().filter_map(|ancestor| ancestor.CurrentBoundingRectangle().ok()).collect();
        let offscreen = element.CurrentIsOffscreen().map(|flag| flag.as_bool()).unwrap_or(false);
        let mut areas = vec![at.frame];
        areas.extend(at.popups.iter().map(|(_, rect)| *rect));
        aim(rect, &clips, &areas, &at.monitors, offscreen).map_err(|missed| match missed {
            Missed::NotVisible => (
                "input_failed".to_string(),
                "The element is not in the visible part of the window; use computer_element_action with scroll_into_view first, then read a new tree or click it again.".to_string(),
            )
                .into(),
            Missed::Offscreen => (
                "offscreen".to_string(),
                "The element is in a part of the window that is outside every screen, so no input was sent. Ask the user to move the window back onto a screen.".to_string(),
            )
                .into(),
        })
    }
}

/// Whether UI Automation's `error` says the element no longer exists: it was removed from its
/// app's tree, or its app is gone.
fn element_gone(error: &windows::core::Error) -> bool {
    const UIA_E_ELEMENTNOTAVAILABLE: u32 = 0x8004_0201;
    const RPC_E_DISCONNECTED: u32 = 0x8001_0108;
    const RPC_S_SERVER_UNAVAILABLE: u32 = 0x8007_06BA;
    [UIA_E_ELEMENTNOTAVAILABLE, RPC_E_DISCONNECTED, RPC_S_SERVER_UNAVAILABLE].contains(&(error.code().0 as u32))
}

/// How far up the Control View a parent chain is followed (to the target, or to a popup). App
/// trees can be deep (Chromium nests many levels of containers), so there is room to spare.
const PARENT_LIMIT: usize = 256;

/// `element`'s Control View ancestors, nearest first, below the first of `roots` (the target, then
/// its popups) that its parent chain reaches, or None when it reaches none: the element is gone,
/// or no longer in the target or its popups.
unsafe fn ancestors_below(
    automation: &IUIAutomation,
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    roots: &[IUIAutomationElement],
) -> Option<Vec<IUIAutomationElement>> {
    let is_root = |candidate: &IUIAutomationElement| roots.iter().any(|root| automation.CompareElements(candidate, root).is_ok_and(|same| same.as_bool()));
    if is_root(element) {
        return Some(Vec::new());
    }
    let mut ancestors = Vec::new();
    let mut current = element.clone();
    for _ in 0..PARENT_LIMIT {
        let parent = walker.GetParentElement(&current).ok()?;
        if is_root(&parent) {
            return Some(ancestors);
        }
        ancestors.push(parent.clone());
        current = parent;
    }
    None
}

/// Why an element click has no point.
#[derive(Debug, PartialEq)]
enum Missed {
    /// Not in the visible part of the target or its popups (scrolled out, or out of its list).
    NotVisible,
    /// In the target or a popup, but where that is outside every monitor.
    Offscreen,
}

fn intersect(a: &RECT, b: &RECT) -> Option<RECT> {
    let rect = RECT { left: a.left.max(b.left), top: a.top.max(b.top), right: a.right.min(b.right), bottom: a.bottom.min(b.bottom) };
    (rect.left < rect.right && rect.top < rect.bottom).then_some(rect)
}

/// Where a click on an element whose bounding rectangle is `rect` (screen pixels) goes: the
/// centre of its part that is in one of `areas` (the target's visible frame and its popups'
/// frames) and on one of `monitors`, the largest such part.
///
/// Only that part and UI Automation decide whether there is a point: an element without an area,
/// or outside the target and its popups, is `NotVisible`; one only where they are off every screen
/// is `Offscreen` (whatever UI Automation says of it); then `offscreen` (UI Automation's
/// `IsOffscreen`, which a list says of a row scrolled out of it) is `NotVisible`. The rectangles of
/// its ancestors (`clips`, nearest first; empty ones, which some containers report, passed over)
/// only choose the point: they cut the element one at a time (the part of a row its list shows),
/// and one the part lies wholly outside of is passed over rather than hiding it: a tree item's
/// rectangle is its own row, not the items under it, and a menu may lie outside the button that
/// holds it. What is left is clicked where it is in the target and on a screen; when none of it
/// is, the element's whole visible part is.
fn aim(rect: RECT, clips: &[RECT], areas: &[RECT], monitors: &[RECT], offscreen: bool) -> Result<(i32, i32), Missed> {
    let empty = |rect: &RECT| rect.right <= rect.left || rect.bottom <= rect.top;
    if empty(&rect) {
        return Err(Missed::NotVisible);
    }
    let shown = |part: &RECT| -> Vec<RECT> { areas.iter().filter_map(|area| intersect(part, area)).collect() };
    let on_screen = |parts: &[RECT]| -> Vec<RECT> { parts.iter().flat_map(|part| monitors.iter().filter_map(move |monitor| intersect(part, monitor))).collect() };
    let in_target = shown(&rect);
    if in_target.is_empty() {
        return Err(Missed::NotVisible);
    }
    let visible = on_screen(&in_target);
    if visible.is_empty() {
        return Err(Missed::Offscreen);
    }
    if offscreen {
        return Err(Missed::NotVisible);
    }
    let within_ancestors = clips.iter().filter(|clip| !empty(clip)).fold(rect, |part, clip| intersect(&part, clip).unwrap_or(part));
    let parts = Some(on_screen(&shown(&within_ancestors))).filter(|parts| !parts.is_empty()).unwrap_or(visible);
    let size = |rect: &RECT| (rect.right - rect.left) as i64 * (rect.bottom - rect.top) as i64;
    let best = parts.into_iter().max_by_key(size).ok_or(Missed::NotVisible)?;
    Ok((best.left + (best.right - best.left) / 2, best.top + (best.bottom - best.top) / 2))
}

/// An element in the walked tree.
trait Node: Sized {
    /// Its children, in order, fetched at once (ticket 69: one call per parent, so the walk knows
    /// how many there are without visiting them, and checks its limits between the calls). Empty
    /// when they cannot be read.
    fn children(&self) -> Vec<Self>;
}

/// Where a walk starts: below an element (the window), or at an element itself without its
/// siblings (a popup, whose siblings are other apps' windows).
enum Root<N> {
    ChildrenOf(N),
    Itself(N),
}

/// What the walk does with an element.
enum Seen {
    /// Not shown, and neither is anything below it (offscreen, empty, outside the window).
    Skip,
    /// Not shown itself (an unnamed container); its children take its place one level up.
    Hidden,
    /// `matched`: its name or value holds a query's text (ticket 63; false without a query).
    Shown { line: Value, actionable: bool, focused: bool, matched: bool },
}

impl<N: Node> Root<N> {
    /// What the walk visits first: the window's children, or the popup itself.
    fn start(self) -> std::vec::IntoIter<N> {
        match self {
            Root::ChildrenOf(node) => node.children().into_iter(),
            Root::Itself(node) => vec![node].into_iter(),
        }
    }
}

/// The children of one element on the path from a root to the element being visited: the walk
/// keeps the whole chain, with the count of children visited per parent.
struct Level<N> {
    /// The children not visited yet.
    next: std::vec::IntoIter<N>,
    visited: usize,
    /// Depth of these children in the shown tree.
    depth: usize,
    /// The nearest shown ancestor's line, where children left out here are counted; None at the top.
    shown: Option<usize>,
}

/// Depth-first over each root in turn, in order, within the limits: `CHILD_LIMIT` children per
/// parent (the rest counted as omitted on the nearest shown ancestor), `DEPTH_LIMIT` levels of
/// the shown tree (children below counted as omitted, `truncated: depth`), and `NODE_LIMIT`
/// lines (`truncated: nodes`). `cut_short` is asked before each element, so between any two
/// fetches of children (`Node::children`); the children past `CHILD_LIMIT` are counted from their
/// parent's fetch, not visited.
fn walk_tree<N: Node, E>(
    roots: Vec<Root<N>>,
    mut describe: impl FnMut(&N, usize) -> Seen,
    keep: impl Fn(&N) -> E,
    walked: &Mutex<Walked<E>>,
    cut_short: impl Fn() -> Option<Ended>,
) -> Ended {
    let omit = |shown: Option<usize>, omitted: usize| {
        let mut walked = lock(walked);
        match shown {
            // A line that is gone was taken with the rest (`get_ui_tree` ran out of time), and
            // what is counted now goes nowhere.
            Some(line) => {
                if let Some(line) = walked.elements.get_mut(line) {
                    let before = line["omitted_children"].as_u64().unwrap_or(0);
                    line["omitted_children"] = json!(before + omitted as u64);
                }
            }
            None => walked.omitted_children += omitted,
        }
    };
    for root in roots {
        let mut path = vec![Level { next: root.start(), visited: 0, depth: 0, shown: None }];
        while let Some(level) = path.last_mut() {
            let Some(element) = level.next.next() else {
                path.pop();
                continue;
            };
            if let Some(ended) = cut_short() {
                return ended;
            }
            if level.visited == CHILD_LIMIT {
                // This child and the ones after it.
                omit(level.shown, 1 + level.next.len());
                path.pop();
                continue;
            }
            if lock(walked).elements.len() >= NODE_LIMIT {
                lock(walked).truncated.push("nodes");
                return Ended::Whole;
            }
            level.visited += 1;
            let (depth, parent_shown) = (level.depth, level.shown);
            let (child_depth, shown) = match describe(&element, depth) {
                Seen::Skip => continue,
                Seen::Hidden => (depth, parent_shown),
                Seen::Shown { mut line, actionable, focused, .. } => {
                    let mut walked = lock(walked);
                    if actionable {
                        walked.refs.push(keep(&element));
                        let index = walked.refs.len();
                        line["index"] = json!(index);
                        if focused {
                            walked.focused_index = Some(index);
                        }
                    }
                    walked.elements.push(line);
                    (depth + 1, Some(walked.elements.len() - 1))
                }
            };
            let children = element.children();
            if child_depth >= DEPTH_LIMIT {
                if !children.is_empty() {
                    omit(shown, children.len());
                    let mut walked = lock(walked);
                    if !walked.truncated.contains(&"depth") {
                        walked.truncated.push("depth");
                    }
                }
                continue;
            }
            path.push(Level { next: children.into_iter(), visited: 0, depth: child_depth, shown });
        }
    }
    Ended::Whole
}

/// A shown element on the path of a query walk to the element being visited: not in the tree
/// until it matches, or a match below it needs it as an ancestor.
struct Pending<E> {
    line: Value,
    /// Its reference, when it is numbered.
    keep: Option<E>,
    focused: bool,
    /// Its line, once in the tree.
    at: Option<usize>,
    /// Its matching children in the tree, against `CHILD_LIMIT`.
    matches: usize,
    /// Its matching children left out past `CHILD_LIMIT`.
    omitted: usize,
    /// It matched past its parent's `CHILD_LIMIT` and is counted there as omitted.
    omitted_there: bool,
}

/// The children of one element on the path of a query walk.
struct Branch<N> {
    /// The children not visited yet.
    next: std::vec::IntoIter<N>,
    /// Depth of these children in the shown tree.
    depth: usize,
    /// They are the children of the last shown element on the path (not of an unnamed container).
    owned: bool,
}

/// Ticket 63: the walk for a query, over each root in turn like `walk_tree`. It visits every
/// child of a parent, within `DEPTH_LIMIT` (`truncated: depth`) and the time `cut_short` allows,
/// and puts in the tree only the elements `describe` says matched and their shown ancestors, as
/// soon as it reaches them, so a walk cut short keeps what it found. The limits on what is shown
/// apply to that: `CHILD_LIMIT` matches per parent (the rest counted as omitted there; an
/// ancestor of a match takes no place), and `NODE_LIMIT` lines, where the walk stops
/// (`truncated: nodes`).
fn walk_matching<N: Node, E>(
    roots: Vec<Root<N>>,
    mut describe: impl FnMut(&N, usize) -> Seen,
    keep: impl Fn(&N) -> E,
    walked: &Mutex<Walked<E>>,
    cut_short: impl Fn() -> Option<Ended>,
) -> Ended {
    for root in roots {
        let mut chain: Vec<Pending<E>> = Vec::new();
        // Matches at the top of this root, against `CHILD_LIMIT`.
        let mut top_matches = 0;
        let mut path = vec![Branch { next: root.start(), depth: 0, owned: false }];
        while let Some(branch) = path.last_mut() {
            let Some(element) = branch.next.next() else {
                if path.pop().is_some_and(|done| done.owned) {
                    chain.pop();
                }
                continue;
            };
            if let Some(ended) = cut_short() {
                return ended;
            }
            let depth = branch.depth;
            let (line, actionable, focused, matched) = match describe(&element, depth) {
                Seen::Skip => continue,
                Seen::Hidden => {
                    path.push(Branch { next: element.children().into_iter(), depth, owned: false });
                    continue;
                }
                Seen::Shown { line, actionable, focused, matched } => (line, actionable, focused, matched),
            };
            let parent = chain.len().checked_sub(1);
            chain.push(Pending { line, keep: actionable.then(|| keep(&element)), focused, at: None, matches: 0, omitted: 0, omitted_there: false });
            if matched {
                let shown = match parent {
                    Some(parent) => chain[parent].matches,
                    None => top_matches,
                };
                if shown < CHILD_LIMIT {
                    if !put(&mut chain, walked) {
                        return Ended::Whole;
                    }
                    match parent {
                        Some(parent) => chain[parent].matches += 1,
                        None => top_matches += 1,
                    }
                } else {
                    let mut walked = lock(walked);
                    match parent {
                        Some(parent) => {
                            chain[parent].omitted += 1;
                            show_omitted(&mut walked, &chain[parent]);
                        }
                        None => walked.omitted_children += 1,
                    }
                    if let Some(me) = chain.last_mut() {
                        me.omitted_there = true;
                    }
                }
            }
            let children = element.children();
            if depth + 1 >= DEPTH_LIMIT {
                if !children.is_empty() {
                    let mut walked = lock(walked);
                    if !walked.truncated.contains(&"depth") {
                        walked.truncated.push("depth");
                    }
                }
                // Popped at once, with this element.
                path.push(Branch { next: Vec::new().into_iter(), depth: depth + 1, owned: true });
                continue;
            }
            path.push(Branch { next: children.into_iter(), depth: depth + 1, owned: true });
        }
    }
    Ended::Whole
}

/// Puts the elements of `chain` not in the tree yet (the last one, a match, and the ancestors
/// it needs) in it, numbering those that are numbered; an ancestor counted as an omitted match
/// is no longer. False, with `truncated: nodes`, when that would pass `NODE_LIMIT` lines.
fn put<E>(chain: &mut [Pending<E>], walked: &Mutex<Walked<E>>) -> bool {
    let mut walked = lock(walked);
    // An element in the tree has its ancestors there too, so those missing are the last ones.
    let from = chain.iter().position(|pending| pending.at.is_none()).unwrap_or(chain.len());
    if walked.elements.len() + (chain.len() - from) > NODE_LIMIT {
        walked.truncated.push("nodes");
        return false;
    }
    for at in from..chain.len() {
        if std::mem::take(&mut chain[at].omitted_there) {
            match at.checked_sub(1) {
                Some(parent) => {
                    chain[parent].omitted -= 1;
                    show_omitted(&mut walked, &chain[parent]);
                }
                None => walked.omitted_children = walked.omitted_children.saturating_sub(1),
            }
        }
        let pending = &mut chain[at];
        let mut line = std::mem::take(&mut pending.line);
        if let Some(reference) = pending.keep.take() {
            walked.refs.push(reference);
            let index = walked.refs.len();
            line["index"] = json!(index);
            if pending.focused {
                walked.focused_index = Some(index);
            }
        }
        walked.elements.push(line);
        pending.at = Some(walked.elements.len() - 1);
    }
    true
}

/// Writes the omitted matches of `pending` on its line. A line that is gone was taken with the
/// rest (`get_ui_tree` ran out of time), and the count goes nowhere.
fn show_omitted<E>(walked: &mut Walked<E>, pending: &Pending<E>) {
    let Some(line) = pending.at.and_then(|line| walked.elements.get_mut(line)) else { return };
    match pending.omitted {
        0 => {
            if let Some(line) = line.as_object_mut() {
                line.remove("omitted_children");
            }
        }
        omitted => line["omitted_children"] = json!(omitted),
    }
}

/// Ticket 69: the properties `describe` reads (`Cached::of`), brought with each element.
const TREE_PROPERTIES: [UIA_PROPERTY_ID; 13] = [
    UIA_IsOffscreenPropertyId,
    UIA_BoundingRectanglePropertyId,
    UIA_ControlTypePropertyId,
    UIA_NamePropertyId,
    UIA_IsEnabledPropertyId,
    UIA_IsPasswordPropertyId,
    UIA_HasKeyboardFocusPropertyId,
    UIA_IsKeyboardFocusablePropertyId,
    UIA_ValueIsReadOnlyPropertyId,
    UIA_ToggleToggleStatePropertyId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_SelectionItemIsSelectedPropertyId,
    UIA_RangeValueValuePropertyId,
];

/// Ticket 69: the patterns whose presence (and cached properties above) `describe` reads.
const TREE_PATTERNS: [UIA_PATTERN_ID; 7] = [
    UIA_ValuePatternId,
    UIA_InvokePatternId,
    UIA_TogglePatternId,
    UIA_ExpandCollapsePatternId,
    UIA_SelectionItemPatternId,
    UIA_ScrollItemPatternId,
    UIA_RangeValuePatternId,
];

/// Ticket 69 (spec-uia "tree-read performance"): the cache request a tree read fetches elements with: the
/// Control View (its tree filter is the condition `ControlViewWalker` walks by, and the cached
/// children of an element are its children in the view of that filter, so the same children in
/// the same order as the walker's first child and next siblings), at
/// `scope` (`TreeScope_Children`: a parent's children in one call; `TreeScope_Element`: a popup
/// itself), with what `describe` reads. Never `TreeScope_Subtree`: one call for the whole tree
/// could not stop for the time limit or the limits per level. The elements are full references
/// (`AutomationElementMode_Full`, the default, set here to be sure): the numbered ones are kept for
/// the element actions, which read their live state again (`Current…`) before they act.
///
/// The Value pattern's value (`UIA_ValueValuePropertyId`) is not in the request: a password field's
/// text would then cross from its app into this process with the rest. `describe` reads a value
/// apart, and only for an element that is no password field (`read_value`).
unsafe fn tree_request(automation: &IUIAutomation, scope: TreeScope) -> windows::core::Result<IUIAutomationCacheRequest> {
    let request = automation.CreateCacheRequest()?;
    request.SetTreeScope(scope)?;
    request.SetTreeFilter(&automation.ControlViewCondition()?)?;
    request.SetAutomationElementMode(AutomationElementMode_Full)?;
    for property in TREE_PROPERTIES {
        request.AddProperty(property)?;
    }
    for pattern in TREE_PATTERNS {
        request.AddPattern(pattern)?;
    }
    Ok(request)
}

/// A UI Automation element in the Control View, with what `describe` reads of it in its cache.
struct UiaNode<'a> {
    element: IUIAutomationElement,
    /// `tree_request` at `TreeScope_Children`.
    children: &'a IUIAutomationCacheRequest,
}

impl Node for UiaNode<'_> {
    /// One call into the app: the element's Control View children with their properties.
    fn children(&self) -> Vec<Self> {
        unsafe {
            let Ok(found) = self.element.BuildUpdatedCache(self.children).and_then(|updated| updated.GetCachedChildren()) else { return Vec::new() };
            let count = found.Length().unwrap_or(0);
            (0..count).filter_map(|at| found.GetElement(at).ok()).map(|element| UiaNode { element, children: self.children }).collect()
        }
    }
}

/// The element's value, read from its app now (ticket 69: not in the cache request, see
/// `tree_request`); `describe` asks for it only where there is no password field.
unsafe fn read_value(element: &IUIAutomationElement) -> Option<String> {
    let value: IUIAutomationValuePattern = element.GetCachedPatternAs(UIA_ValuePatternId).ok()?;
    value.CurrentValue().ok().map(|text| text.to_string())
}

/// The Control View below the window, then each of the window's own `popups` (handle and visible
/// frame) that is not already part of the window's tree (a dropdown list UI Automation puts under
/// its combo box). An element entirely outside the window and those popups is skipped. With a
/// `query` (folded) only its matches and their ancestors are kept (`walk_matching`).
fn walk(automation: &IUIAutomation, scope: &Scope, query: Option<&str>, walked: &Mutex<Walked<Element>>, deadline: Instant) -> Ended {
    let Scope { target, frame, popups, chromium, title } = scope;
    let (target, frame) = (*target, *frame);
    let hwnd = HWND(target as *mut core::ffi::c_void);
    let found = unsafe {
        automation.ElementFromHandle(hwnd).and_then(|root| {
            Ok((root, automation.ControlViewWalker()?, tree_request(automation, TreeScope_Children)?, tree_request(automation, TreeScope_Element)?))
        })
    };
    let (root, walker, children, itself) = match found {
        Ok(found) => found,
        Err(error) => return Ended::Failed(error.to_string()),
    };
    // Ticket 66: the first query is what makes a Chromium app build its web content's tree, so give
    // it the wait before the walk; the walk then reads what is there.
    // Ticket 71: a window found empty by an earlier read (same title) does not wait again; whether
    // it is still empty is read from the Documents the walk passes (`main_documents`).
    let found_empty = *chromium && lock(&EMPTY_WAITS).knows(target, title);
    if found_empty {
        lock(walked).web_content_empty = true;
    } else if *chromium {
        // A search that has not finished by then answers "empty", so it keeps off the walk's time as the wait does.
        let until = deadline.checked_sub(WEB_MARGIN).unwrap_or_else(Instant::now);
        let known = RefCell::new(Vec::new());
        match wait_for_web_content(|| unsafe { web_content_ready(automation, &walker, &root, until, &known) }, deadline) {
            Ok(empty) => {
                lock(walked).web_content_empty = empty;
                if empty {
                    lock(&EMPTY_WAITS).remember(target, title, window_is_open);
                } else {
                    lock(&EMPTY_WAITS).forget(target);
                }
            }
            Err(Stopped) => return Ended::Stopped,
        }
    }
    // Only kept for a window found empty before: its Documents' areas and elements.
    let main_documents = RefCell::new(Vec::new());
    let mut area = vec![frame];
    area.extend(popups.iter().map(|(_, rect)| *rect));
    let mut roots = vec![Root::ChildrenOf(UiaNode { element: root.clone(), children: &children })];
    for (popup, _) in popups {
        if esc::stopped() {
            return Ended::Stopped;
        }
        // The popup is described itself, so it comes with its properties.
        let Ok(element) = (unsafe { automation.ElementFromHandleBuildCache(HWND(*popup as *mut core::ffi::c_void), &itself) }) else { continue };
        if !unsafe { below(automation, &walker, &element, &root) } {
            roots.push(Root::Itself(UiaNode { element, children: &children }));
        }
    }
    let cut_short = || {
        if esc::stopped() {
            Some(Ended::Stopped)
        } else if Instant::now() >= deadline {
            Some(Ended::OutOfTime)
        } else {
            None
        }
    };
    let seen = |node: &UiaNode, depth| unsafe {
        let cached = Cached::of(&node.element);
        if found_empty && cached.control == Some(UIA_DocumentControlTypeId) {
            main_documents.borrow_mut().push((area_of(cached.rect), node.element.clone()));
        }
        describe(&cached, || read_value(&node.element), depth, frame, &area, query)
    };
    let keep = |node: &UiaNode| Element(node.element.clone());
    let ended = match query {
        Some(_) => walk_matching(roots, seen, keep, walked, cut_short),
        None => walk_tree(roots, seen, keep, walked, cut_short),
    };
    // The web content of a window found empty before: ready now if the largest Document the walk
    // met has children. A walk that ended early has met what it met; the answer stays "empty" then.
    if found_empty && !matches!(ended, Ended::Stopped) && Instant::now() < deadline {
        let documents: Vec<(i64, bool)> = main_documents
            .borrow()
            .iter()
            .filter_map(|(area, document)| Some((*area, unsafe { document_shows_content(&walker, document) }?)))
            .collect();
        if main_content_ready(&documents) {
            lock(walked).web_content_empty = false;
            lock(&EMPTY_WAITS).forget(target);
        }
    }
    ended
}

/// Ticket 66: the class Chromium and Electron give their windows.
const CHROMIUM_WINDOW: &str = "Chrome_WidgetWin_1";
/// Elements at most that the search for a web area looks at (the whole window of a browser is a few hundred).
const WEB_AREA_SEARCH: usize = 3000;

/// Whether a window is a Chromium or Electron one: its class, or the class of one of its child
/// windows (a web view inside another app's window), is `Chrome_WidgetWin_1`. Any other window is
/// read exactly as before.
fn is_chromium(target_class: &str, child_classes: &[String]) -> bool {
    let chromium = |class: &str| class.eq_ignore_ascii_case(CHROMIUM_WINDOW);
    chromium(target_class) || child_classes.iter().any(|class| chromium(class))
}

fn class_of(window: HWND) -> String {
    let mut buffer = [0u16; 256];
    let length = unsafe { GetClassNameW(window, &mut buffer) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..length])
}

/// `is_chromium` of the target window as it is now. Window handles only: no UI Automation call, so
/// a window that is not Chromium costs nothing more than this.
fn is_chromium_window(target: HWND) -> bool {
    unsafe extern "system" fn collect(child: HWND, list: LPARAM) -> BOOL {
        // SAFETY: `list` is the address of the vector below, which outlives the enumeration.
        unsafe { &mut *(list.0 as *mut Vec<String>) }.push(class_of(child));
        BOOL(1)
    }
    let mut classes: Vec<String> = Vec::new();
    // The enumeration ends early only when the window is gone; then there is nothing to wait for.
    let _ = unsafe { EnumChildWindows(Some(target), Some(collect), LPARAM(&mut classes as *mut Vec<String> as isize)) };
    is_chromium(&class_of(target), &classes)
}

/// Whether `document` shows content in the Control View, as the tree will: it is an element there
/// and has children. None when the element is gone (Chromium builds a new tree and drops the old
/// elements).
unsafe fn document_shows_content(control: &IUIAutomationTreeWalker, document: &IUIAutomationElement) -> Option<bool> {
    let is_control = document.CurrentIsControlElement().ok()?.as_bool();
    Some(is_control && control.GetFirstChildElement(document).is_ok())
}

/// Whether the main web content of the window below `root` is there: the largest web area (a
/// Document element, `main_content_ready`) has children in the Control View, as the tree will show
/// them. Until Chromium has built its web content's tree (the first query is what starts it) there
/// is none: the web area is not in the Control View at all, or is there empty, so the search looks
/// in the Raw View. That search walks the whole window (hundreds of elements in a browser: about
/// half a second), and needs every Document to find the largest, so it does not look below a
/// Document (its content can be thousands of elements); the Documents it finds are kept in
/// `known` with their areas and the next look only asks them again, searching anew when there
/// were none or one is gone. False too when `until` passes first, or a stop comes.
unsafe fn web_content_ready(
    automation: &IUIAutomation,
    control: &IUIAutomationTreeWalker,
    root: &IUIAutomationElement,
    until: Instant,
    known: &RefCell<Vec<(i64, IUIAutomationElement)>>,
) -> bool {
    let mut search = known.borrow().is_empty();
    let mut states = Vec::new();
    for (area, document) in known.borrow().iter() {
        match document_shows_content(control, document) {
            Some(has_children) => states.push((*area, has_children)),
            None => search = true,
        }
    }
    if !search {
        return main_content_ready(&states);
    }
    let Ok(raw) = automation.RawViewWalker() else { return false };
    let mut documents = Vec::new();
    let mut pending = vec![root.clone()];
    let mut looked_at = 0;
    while let Some(element) = pending.pop() {
        looked_at += 1;
        if looked_at > WEB_AREA_SEARCH || Instant::now() >= until || esc::stopped() {
            return false;
        }
        if element.CurrentControlType().is_ok_and(|kind| kind == UIA_DocumentControlTypeId) {
            documents.push((area_of(element.CurrentBoundingRectangle().ok()), element));
            continue;
        }
        let mut child = raw.GetFirstChildElement(&element).ok();
        while let Some(found) = child {
            child = raw.GetNextSiblingElement(&found).ok();
            pending.push(found);
        }
    }
    let states: Vec<(i64, bool)> = documents.iter().filter_map(|(area, document)| Some((*area, document_shows_content(control, document)?))).collect();
    *known.borrow_mut() = documents;
    main_content_ready(&states)
}

/// Whether `element` is below `root` in the Control View.
unsafe fn below(automation: &IUIAutomation, walker: &IUIAutomationTreeWalker, element: &IUIAutomationElement, root: &IUIAutomationElement) -> bool {
    let mut current = element.clone();
    for _ in 0..PARENT_LIMIT {
        let Ok(parent) = walker.GetParentElement(&current) else { return false };
        if automation.CompareElements(&parent, root).is_ok_and(|same| same.as_bool()) {
            return true;
        }
        current = parent;
    }
    false
}

/// Text cut to `limit` characters, with its full length in characters when it was longer.
fn cut_text(text: String, limit: usize) -> (String, Option<usize>) {
    let length = text.chars().count();
    if length <= limit {
        return (text, None);
    }
    (text.chars().take(limit).collect(), Some(length))
}

fn overlaps(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

/// What `describe` reads of an element (ticket 69): what its cache request (`tree_request`)
/// brought, read in this process without a call into the app, each as the earlier reads one at a
/// time took it when it could not be read. Its value is not among it (`read_value`).
#[derive(Default)]
struct Cached {
    /// IsOffscreen; unreadable: not offscreen.
    offscreen: bool,
    rect: Option<RECT>,
    control: Option<UIA_CONTROLTYPE_ID>,
    /// Unreadable: empty.
    name: String,
    /// The Value pattern's IsReadOnly (unreadable: read-only); None without the pattern.
    value_read_only: Option<bool>,
    invoke: bool,
    /// The Toggle pattern's state (unreadable: off); None without the pattern.
    toggle: Option<ToggleState>,
    /// The ExpandCollapse state; None without the pattern, when unreadable, or for a leaf
    /// (nothing to expand: no state, no action).
    expand: Option<ExpandCollapseState>,
    /// The SelectionItem pattern's IsSelected (unreadable: not selected); None without the pattern.
    selected: Option<bool>,
    scroll_item: bool,
    /// Unreadable: enabled.
    enabled: bool,
    /// IsPassword; None when it could not be read.
    password: Option<bool>,
    focused: bool,
    focusable: bool,
    /// The RangeValue pattern's value; None without the pattern or when unreadable.
    range: Option<f64>,
}

impl Cached {
    /// From `element`'s cache, filled by `tree_request`.
    unsafe fn of(element: &IUIAutomationElement) -> Self {
        let flag = |read: windows::core::Result<BOOL>| read.map(|flag| flag.as_bool());
        let value: Option<IUIAutomationValuePattern> = element.GetCachedPatternAs(UIA_ValuePatternId).ok();
        let toggle: Option<IUIAutomationTogglePattern> = element.GetCachedPatternAs(UIA_TogglePatternId).ok();
        let expand: Option<IUIAutomationExpandCollapsePattern> = element.GetCachedPatternAs(UIA_ExpandCollapsePatternId).ok();
        let selection: Option<IUIAutomationSelectionItemPattern> = element.GetCachedPatternAs(UIA_SelectionItemPatternId).ok();
        let range: Option<IUIAutomationRangeValuePattern> = element.GetCachedPatternAs(UIA_RangeValuePatternId).ok();
        Cached {
            offscreen: flag(element.CachedIsOffscreen()).unwrap_or(false),
            rect: element.CachedBoundingRectangle().ok(),
            control: element.CachedControlType().ok(),
            name: element.CachedName().map(|name| name.to_string()).unwrap_or_default(),
            value_read_only: value.map(|value| flag(value.CachedIsReadOnly()).unwrap_or(true)),
            invoke: element.GetCachedPattern(UIA_InvokePatternId).is_ok(),
            toggle: toggle.map(|toggle| toggle.CachedToggleState().unwrap_or(ToggleState_Off)),
            expand: expand.and_then(|expand| expand.CachedExpandCollapseState().ok()).filter(|state| *state != ExpandCollapseState_LeafNode),
            selected: selection.map(|selection| flag(selection.CachedIsSelected()).unwrap_or(false)),
            scroll_item: element.GetCachedPattern(UIA_ScrollItemPatternId).is_ok(),
            enabled: flag(element.CachedIsEnabled()).unwrap_or(true),
            password: flag(element.CachedIsPassword()).ok(),
            focused: flag(element.CachedHasKeyboardFocus()).unwrap_or(false),
            focusable: flag(element.CachedIsKeyboardFocusable()).unwrap_or(false),
            range: range.and_then(|range| range.CachedValue().ok()),
        }
    }
}

/// The element as the walk sees it, from what its cache holds (`Cached`), and its value, which
/// `read_value` reads from the app only for an element with a Value pattern that is no password
/// field. Positions are relative to the window's visible `frame`; `area` is the frame and its
/// popups' frames. With a `query` (folded by `fold`), a shown element is `matched` when its whole
/// name or value (before either is cut) holds it; a password box's value is never read, so it
/// never takes part.
fn describe(element: &Cached, read_value: impl FnOnce() -> Option<String>, depth: usize, frame: RECT, area: &[RECT], query: Option<&str>) -> Seen {
    if element.offscreen {
        return Seen::Skip;
    }
    let Some(rect) = element.rect else { return Seen::Skip };
    if rect.right <= rect.left || rect.bottom <= rect.top || !area.iter().any(|shown| overlaps(&rect, shown)) {
        return Seen::Skip;
    }
    let Some(control) = element.control else { return Seen::Hidden };
    let role = role_name(control);
    let mut matched = query.is_some_and(|query| holds(&element.name, query));
    let (name, _) = cut_text(element.name.clone(), NAME_CHARS);
    let read_only = element.value_read_only;
    let settable = read_only == Some(false);
    let Cached { invoke, toggle, expand: expand_state, selected, scroll_item, enabled, focused, focusable, .. } = *element;
    let actionable = INTERACTIVE.contains(&control.0) || invoke || toggle.is_some() || settable || expand_state.is_some() || selected.is_some() || scroll_item;
    if !actionable {
        if name.is_empty() {
            return Seen::Hidden;
        }
        return Seen::Shown { line: json!({ "depth": depth, "role": role, "name": name, "states": [], "actions": [] }), actionable: false, focused: false, matched };
    }
    // Only a value can give a password away, so it is judged where there is one. Fail closed: an
    // element whose IsPassword could not be read is treated as a password box. IsPassword is in the
    // cache request, so this mostly guards against it being dropped from there; a provider that
    // cannot supply it gives the cache's default, and the value read itself then fails or is empty.
    let password = read_only.is_some() && element.password.unwrap_or(true);

    let mut states = Vec::new();
    if focused {
        states.push("focused");
    }
    if !enabled {
        states.push("disabled");
    }
    if selected == Some(true) {
        states.push("selected");
    }
    if let Some(state) = toggle {
        states.push(toggle_name(state));
    }
    match expand_state {
        Some(state) if state == ExpandCollapseState_Expanded => states.push("expanded"),
        Some(state) if state == ExpandCollapseState_PartiallyExpanded => states.push("partially expanded"),
        Some(_) => states.push("collapsed"),
        None => {}
    }
    if read_only == Some(true) {
        states.push("read-only");
    }
    if password {
        states.push("password");
    }
    // A disabled element is listed with no actions.
    let mut actions = Vec::new();
    if enabled {
        actions.push("click");
        if settable {
            actions.push("set_value");
        }
        // Only what the current state allows (`Abilities::allows` by state).
        let abilities = Abilities { invoke, toggle: toggle.is_some(), expand: expand_state, select: selected.is_some(), scroll_item, focusable };
        actions.extend(abilities.actions(true));
    }
    let mut line = json!({
        "depth": depth,
        "role": role,
        "states": states,
        "actions": actions,
        "bounds": { "x": rect.left - frame.left, "y": rect.top - frame.top, "width": rect.right - rect.left, "height": rect.bottom - rect.top },
    });
    if !name.is_empty() {
        line["name"] = json!(name);
    }
    // A password box's value is never read.
    if !password {
        let text = match read_only {
            Some(_) => read_value(),
            None if !RANGED.contains(&control.0) => None,
            None => element.range.map(|number| number.to_string()),
        };
        if let Some(text) = text {
            matched = matched || query.is_some_and(|query| holds(&text, query));
            let (text, length) = cut_text(text, VALUE_CHARS);
            line["value"] = json!(text);
            if let Some(length) = length {
                line["value_length"] = json!(length);
            }
        }
    }
    Seen::Shown { line, actionable: true, focused, matched }
}

/// A query's text as it is compared (ticket 63): lowercased, which also folds non-ASCII letters
/// (Ä, Σ) and leaves text without case (Chinese) as it is.
fn fold(text: &str) -> String {
    text.to_lowercase()
}

/// Whether `text` holds the `fold`ed `query`, without regard to case.
fn holds(text: &str, query: &str) -> bool {
    fold(text).contains(query)
}

/// The control type's lowercase English name.
fn role_name(control: UIA_CONTROLTYPE_ID) -> &'static str {
    match control.0 {
        50000 => "button",
        50001 => "calendar",
        50002 => "check box",
        50003 => "combo box",
        50004 => "edit",
        50005 => "hyperlink",
        50006 => "image",
        50007 => "list item",
        50008 => "list",
        50009 => "menu",
        50010 => "menu bar",
        50011 => "menu item",
        50012 => "progress bar",
        50013 => "radio button",
        50014 => "scroll bar",
        50015 => "slider",
        50016 => "spinner",
        50017 => "status bar",
        50018 => "tab",
        50019 => "tab item",
        50020 => "text",
        50021 => "tool bar",
        50022 => "tool tip",
        50023 => "tree",
        50024 => "tree item",
        50025 => "custom",
        50026 => "group",
        50027 => "thumb",
        50028 => "data grid",
        50029 => "data item",
        50030 => "document",
        50031 => "split button",
        50032 => "window",
        50033 => "pane",
        50034 => "header",
        50035 => "header item",
        50036 => "table",
        50037 => "title bar",
        50038 => "separator",
        50039 => "semantic zoom",
        50040 => "app bar",
        _ => "element",
    }
}

/// The element references of recent trees, newest last.
struct TreeCache<E> {
    trees: VecDeque<KeptTree<E>>,
}

struct KeptTree<E> {
    tree_id: String,
    window: isize,
    /// Who had the window handle when the tree was read.
    owner: Owner,
    elements: Vec<E>,
}

impl<E> TreeCache<E> {
    const fn new() -> Self {
        Self { trees: VecDeque::new() }
    }

    /// Keeps a new tree of `window`, dropping that window's trees beyond the newest
    /// `TREES_PER_WINDOW` and every tree of a window that is no longer open or whose handle now
    /// names another window (`current` owner). Returns the dropped trees' elements, for the
    /// caller to release where they may be released.
    fn insert(&mut self, tree_id: String, window: isize, owner: Owner, elements: Vec<E>, current: impl Fn(isize) -> Option<Owner>) -> Vec<Vec<E>> {
        let mut dropped = Vec::new();
        for tree in std::mem::take(&mut self.trees) {
            if current(tree.window) == Some(tree.owner) {
                self.trees.push_back(tree);
            } else {
                dropped.push(tree.elements);
            }
        }
        self.trees.push_back(KeptTree { tree_id, window, owner, elements });
        let kept = self.trees.iter().filter(|tree| tree.window == window).count();
        let mut excess = kept.saturating_sub(TREES_PER_WINDOW);
        for tree in std::mem::take(&mut self.trees) {
            if excess > 0 && tree.window == window {
                excess -= 1;
                dropped.push(tree.elements);
            } else {
                self.trees.push_back(tree);
            }
        }
        dropped
    }

    /// Element `index` (from 1) of tree `tree_id`, for an action on `window`, whose handle `owner`
    /// has now (None: closed). A tree this helper does not have (dropped, or read by an earlier
    /// helper process) is `no_ui_tree`, whatever the server's records say; a tree of another
    /// window or an index past the tree is `invalid_params`; a handle another window has now is
    /// `element_not_found`.
    fn find(&self, tree_id: &str, window: isize, owner: Option<Owner>, index: usize) -> Result<&E, Failure> {
        let Some(tree) = self.trees.iter().find(|tree| tree.tree_id == tree_id) else {
            return Err((
                "no_ui_tree".to_string(),
                format!("UI tree {tree_id} is not available in the helper (dropped for newer trees, or read by an earlier helper), so no input was sent. Read a new tree."),
            )
                .into());
        };
        if tree.window != window {
            return Err(("invalid_params".to_string(), format!("UI tree {tree_id} is of another window, so no input was sent.")).into());
        }
        if owner != Some(tree.owner) {
            return Err(element_not_found());
        }
        // Worded as the server words it (`chooseElement`), which the model sees when it does not
        // know the tree's size.
        index.checked_sub(1).and_then(|at| tree.elements.get(at)).ok_or_else(|| {
            let range = match tree.elements.len() {
                0 => "has no numbered elements".to_string(),
                count => format!("has element_index 1 to {count}"),
            };
            ("invalid_params".to_string(), format!("element_index {index} is not in UI tree {tree_id}, which {range}.")).into()
        })
    }

    #[cfg(test)]
    fn tree_ids(&self, window: isize) -> Vec<&str> {
        self.trees.iter().filter(|tree| tree.window == window).map(|tree| tree.tree_id.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Mutex;

    use serde_json::json;

    use super::{aim, cut_text, walk_matching, walk_tree, Ended, Missed, Node, Root, Seen, TreeCache, Walked};

    /// A tree for the walk tests: each node has a label and children. A label `skip…` is a node
    /// the walk skips with its subtree (offscreen, empty, outside the window), an empty label an
    /// unnamed container (not shown), `b…` a numbered button, anything else a text line.
    struct Arena {
        labels: Vec<String>,
        children: Vec<Vec<usize>>,
        /// The nodes whose children the walk fetched, in order (ticket 69).
        fetched: RefCell<Vec<usize>>,
    }

    impl Arena {
        fn new() -> Self {
            Self { labels: vec!["window".to_string()], children: vec![Vec::new()], fetched: RefCell::new(Vec::new()) }
        }
        fn add(&mut self, parent: usize, label: &str) -> usize {
            self.labels.push(label.to_string());
            self.children.push(Vec::new());
            let id = self.labels.len() - 1;
            self.children[parent].push(id);
            id
        }
    }

    #[derive(Clone)]
    struct Fake(Rc<Arena>, usize);

    impl Node for Fake {
        fn children(&self) -> Vec<Self> {
            self.0.fetched.borrow_mut().push(self.1);
            self.0.children[self.1].iter().map(|&child| Fake(Rc::clone(&self.0), child)).collect()
        }
    }

    fn seen(node: &Fake, depth: usize) -> Seen {
        let label = &node.0.labels[node.1];
        if label.starts_with("skip") {
            return Seen::Skip;
        }
        if label.is_empty() {
            return Seen::Hidden;
        }
        let actionable = label.starts_with('b');
        let role = if actionable { "button" } else { "text" };
        Seen::Shown { line: json!({ "depth": depth, "role": role, "name": label, "states": [], "actions": [] }), actionable, focused: label.ends_with("focused"), matched: label.contains("hit") }
    }

    /// A query walk (ticket 63) where a label holding `hit` matches, with `cut_short` asked
    /// before each element.
    fn run_matching(arena: Arena, popups: &[usize], cut_short: impl Fn() -> Option<Ended>) -> (Ended, Walked<String>) {
        let arena = Rc::new(arena);
        let mut roots = vec![Root::ChildrenOf(Fake(Rc::clone(&arena), 0))];
        roots.extend(popups.iter().map(|&id| Root::Itself(Fake(Rc::clone(&arena), id))));
        let walked = Mutex::new(Walked::default());
        let ended = walk_matching(roots, seen, |node: &Fake| node.0.labels[node.1].clone(), &walked, cut_short);
        (ended, walked.into_inner().unwrap())
    }

    /// Ticket 63: only matches and their ancestors, numbered in walk order (an ancestor keeps its
    /// number), unnamed containers still lifting their children, everything else left out.
    #[test]
    fn a_query_walk_shows_only_matches_and_their_ancestors() {
        let mut arena = Arena::new();
        arena.add(0, "b-before");
        let pane = arena.add(0, "");
        let group = arena.add(pane, "Group");
        let item = arena.add(group, "b-item");
        arena.add(item, "b-hit focused");
        arena.add(item, "b-sibling");
        arena.add(group, "text sibling");
        let skipped = arena.add(0, "skip offscreen hit");
        arena.add(skipped, "b-hit never");
        arena.add(0, "text hit");
        arena.add(0, "b-after");
        let desktop = arena.add(0, "skip desktop");
        let menu = arena.add(desktop, "Menu");
        arena.add(menu, "b-menu item");
        let other = arena.add(desktop, "Other menu");
        arena.add(other, "b-menu hit");
        let (ended, walked) = run_matching(arena, &[menu, other], || None);
        assert!(matches!(ended, Ended::Whole));
        assert_eq!(
            outline(&walked),
            [
                line(0, "Group", None, None),
                line(1, "b-item", Some(1), None),
                line(2, "b-hit focused", Some(2), None),
                line(0, "text hit", None, None),
                line(0, "Other menu", None, None),
                line(1, "b-menu hit", Some(3), None),
            ]
        );
        assert_eq!(walked.refs, ["b-item", "b-hit focused", "b-menu hit"]);
        assert_eq!(walked.focused_index, Some(2));
        assert!(walked.truncated.is_empty(), "{:?}", walked.truncated);
    }

    /// Ticket 63: the walk visits every child of a parent, so a match past the 60th is found; a
    /// parent shows 60 matches and counts the rest as omitted, and ancestors take no place there.
    #[test]
    fn a_query_walk_finds_matches_past_sixty_children_and_shows_sixty_matches_per_parent() {
        let mut arena = Arena::new();
        let list = arena.add(0, "List");
        for number in 1..=70 {
            arena.add(list, &format!("b{number}"));
        }
        arena.add(list, "b-hit 71");
        let many = arena.add(0, "Many");
        for number in 1..=65 {
            arena.add(many, &format!("b-hit {number}"));
        }
        // Ancestors of matches are shown whatever the count: the matching child past the limit
        // that is also an ancestor is shown, and is no longer counted as omitted.
        let nested = arena.add(many, "b-hit 66 holder");
        arena.add(nested, "b-hit 67");
        let (ended, walked) = run_matching(arena, &[], || None);
        assert!(matches!(ended, Ended::Whole));
        let lines = outline(&walked);
        assert_eq!(lines[0], line(0, "List", None, None));
        assert_eq!(lines[1], line(1, "b-hit 71", Some(1), None));
        assert_eq!(lines[2], line(0, "Many", None, Some(5)));
        assert_eq!(lines[3], line(1, "b-hit 1", Some(2), None));
        assert_eq!(lines[62], line(1, "b-hit 60", Some(61), None));
        assert_eq!(lines[63], line(1, "b-hit 66 holder", Some(62), None));
        assert_eq!(lines[64], line(2, "b-hit 67", Some(63), None));
        assert_eq!(lines.len(), 65);
        assert!(walked.truncated.is_empty(), "{:?}", walked.truncated);
    }

    /// Ticket 63: top-level matches past 60 are the tree's own omitted children.
    #[test]
    fn a_query_walk_counts_top_level_matches_past_sixty_on_the_tree() {
        let mut arena = Arena::new();
        for number in 1..=62 {
            arena.add(0, &format!("hit {number}"));
        }
        let (_, walked) = run_matching(arena, &[], || None);
        assert_eq!(walked.elements.len(), 60);
        assert_eq!(walked.omitted_children, 2);
    }

    /// Ticket 63: the depth limit still cuts the walk (`truncated: depth`), and the output stops at
    /// 500 lines of matches and ancestors (`truncated: nodes`).
    #[test]
    fn a_query_walk_keeps_the_depth_and_line_limits() {
        let mut arena = Arena::new();
        let mut parent = 0;
        for level in 0..35 {
            parent = arena.add(parent, &format!("level {level}"));
        }
        arena.add(5, "hit shallow");
        let (_, walked) = run_matching(arena, &[], || None);
        assert_eq!(outline(&walked).last(), Some(&line(5, "hit shallow", None, None)));
        assert_eq!(walked.elements.len(), 6);
        assert_eq!(walked.truncated, ["depth"]);

        let mut arena = Arena::new();
        for group in 1..=9 {
            let parent = arena.add(0, &format!("Group {group}"));
            for number in 1..=60 {
                arena.add(parent, &format!("b-hit {group}-{number}"));
            }
        }
        let (ended, walked) = run_matching(arena, &[], || None);
        assert!(matches!(ended, Ended::Whole));
        assert_eq!(walked.elements.len(), 500);
        assert_eq!(walked.truncated, ["nodes"]);
    }

    /// Ticket 63: a walk cut short keeps what it put in the tree so far.
    #[test]
    fn a_query_walk_cut_short_keeps_the_matches_it_reached() {
        let mut arena = Arena::new();
        for number in 1..=10 {
            arena.add(0, &format!("b-hit {number}"));
        }
        let asked = std::cell::Cell::new(0);
        let (ended, walked) = run_matching(arena, &[], || {
            asked.set(asked.get() + 1);
            (asked.get() > 4).then_some(Ended::OutOfTime)
        });
        assert!(matches!(ended, Ended::OutOfTime));
        assert_eq!(walked.elements.len(), 4);
        assert_eq!(walked.refs.len(), 4);
    }

    /// Ticket 63: a query is compared without regard to case, in any script.
    #[test]
    fn a_query_matches_without_regard_to_case() {
        use super::{fold, holds};
        assert!(holds("Save As…", &fold("save as")));
        assert!(holds("ÄRGER", &fold("ärg")));
        assert!(holds("檔案總管 - 下載", &fold("總管")));
        assert!(!holds("檔案", &fold("下載")));
    }

    fn run(arena: Arena, popups: &[usize]) -> (Ended, Walked<String>) {
        let arena = Rc::new(arena);
        let mut roots = vec![Root::ChildrenOf(Fake(Rc::clone(&arena), 0))];
        roots.extend(popups.iter().map(|&id| Root::Itself(Fake(Rc::clone(&arena), id))));
        let walked = Mutex::new(Walked::default());
        let ended = walk_tree(roots, seen, |node: &Fake| node.0.labels[node.1].clone(), &walked, || None);
        (ended, walked.into_inner().unwrap())
    }

    type Outline = (u64, String, Option<u64>, Option<u64>);

    /// `(depth, name, index, omitted_children)` of each line.
    fn outline(walked: &Walked<String>) -> Vec<Outline> {
        walked
            .elements
            .iter()
            .map(|line| (line["depth"].as_u64().unwrap(), line["name"].as_str().unwrap().to_string(), line["index"].as_u64(), line["omitted_children"].as_u64()))
            .collect()
    }

    fn line(depth: u64, name: &str, index: Option<u64>, omitted: Option<u64>) -> Outline {
        (depth, name.to_string(), index, omitted)
    }

    #[test]
    fn unnamed_containers_lift_their_children_and_skipped_elements_take_their_subtree_along() {
        let mut arena = Arena::new();
        let pane = arena.add(0, "");
        arena.add(pane, "b1");
        let group = arena.add(pane, "Group");
        arena.add(group, "b2");
        let hidden = arena.add(group, "");
        arena.add(hidden, "label");
        let skipped = arena.add(0, "skip offscreen list");
        arena.add(skipped, "b-never");
        arena.add(0, "b-focused");
        let (ended, walked) = run(arena, &[]);
        assert!(matches!(ended, Ended::Whole));
        assert_eq!(
            outline(&walked),
            [line(0, "b1", Some(1), None), line(0, "Group", None, None), line(1, "b2", Some(2), None), line(1, "label", None, None), line(0, "b-focused", Some(3), None)]
        );
        assert_eq!(walked.refs, ["b1", "b2", "b-focused"]);
        assert_eq!(walked.focused_index, Some(3));
        assert!(walked.truncated.is_empty());
    }

    #[test]
    fn a_parent_shows_sixty_children_and_counts_the_rest_as_omitted() {
        let mut arena = Arena::new();
        let list = arena.add(0, "List");
        for number in 1..=70 {
            arena.add(list, &format!("b{number}"));
        }
        // An unnamed container's omitted children go to the nearest shown ancestor.
        let outer = arena.add(0, "Outer");
        let inner = arena.add(outer, "");
        for number in 1..=63 {
            arena.add(inner, &format!("t{number}"));
        }
        let (_, walked) = run(arena, &[]);
        let lines = outline(&walked);
        assert_eq!(lines[0], line(0, "List", None, Some(10)));
        assert_eq!(lines.iter().filter(|line| line.1.starts_with('b')).count(), 60);
        assert_eq!(lines[60], line(1, "b60", Some(60), None));
        assert_eq!(lines[61], line(0, "Outer", None, Some(3)));
        assert_eq!(lines.len(), 62 + 60);
        // The per-parent limit is not a truncation of the whole tree.
        assert!(walked.truncated.is_empty(), "{:?}", walked.truncated);
    }

    /// Ticket 69: a parent's children come in one fetch, so the children past the 60th are
    /// counted from it (as many as before) without being visited, and nothing below them is read.
    /// No parent is fetched twice, and neither is anything a skipped element holds.
    #[test]
    fn children_past_sixty_are_counted_from_one_fetch_without_reading_below_them() {
        let mut arena = Arena::new();
        let list = arena.add(0, "List");
        let mut late = Vec::new();
        for number in 1..=70 {
            let row = arena.add(list, &format!("b{number}"));
            arena.add(row, &format!("inner {number}"));
            if number > 60 {
                late.push(row);
            }
        }
        let skipped = arena.add(0, "skip offscreen");
        arena.add(skipped, "b-never");
        let arena = Rc::new(arena);
        let walked = Mutex::new(Walked::default());
        let ended = walk_tree(vec![Root::ChildrenOf(Fake(Rc::clone(&arena), 0))], seen, |node: &Fake| node.0.labels[node.1].clone(), &walked, || None);
        assert!(matches!(ended, Ended::Whole));
        let walked = walked.into_inner().unwrap();
        let lines = outline(&walked);
        assert_eq!(lines[0], line(0, "List", None, Some(10)));
        assert_eq!(lines.len(), 1 + 60 * 2);
        let fetched = arena.fetched.borrow();
        assert_eq!(fetched.iter().filter(|&&id| id == list).count(), 1, "{fetched:?}");
        let mut unique = fetched.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), fetched.len(), "a parent was fetched twice: {fetched:?}");
        assert!(!fetched.iter().any(|id| late.contains(id) || *id == skipped), "{fetched:?}");
    }

    /// Ticket 69: `describe` works from what the cache brought; the value is read apart, and only
    /// for an element that is no password field. A password field, and one whose IsPassword could
    /// not be read, never has its value read: not shown, and not matched against a query.
    #[test]
    fn a_password_fields_value_is_never_read_and_an_unreadable_password_flag_counts_as_one() {
        use super::{describe, Cached};
        use std::cell::Cell;
        use windows::Win32::UI::Accessibility::UIA_CONTROLTYPE_ID;
        let frame = rect(0, 0, 500, 500);
        let edit = |password| Cached {
            rect: Some(rect(10, 10, 110, 34)),
            control: Some(UIA_CONTROLTYPE_ID(50004)),
            name: "Password".to_string(),
            value_read_only: Some(false),
            enabled: true,
            focusable: true,
            password,
            ..Cached::default()
        };
        for password in [Some(true), None] {
            let reads = Cell::new(0);
            let read = || {
                reads.set(reads.get() + 1);
                Some("CUMCP-secret-42".to_string())
            };
            let Seen::Shown { line, matched, actionable, .. } = describe(&edit(password), read, 0, frame, &[frame], Some("secret-42")) else {
                panic!("the password field is not shown ({password:?})");
            };
            assert_eq!(reads.get(), 0, "the value of a password field was read ({password:?})");
            assert!(actionable && !matched, "{line}");
            assert!(line.get("value").is_none(), "{line}");
            assert!(line["states"].as_array().is_some_and(|states| states.iter().any(|state| state == "password")), "{line}");
            assert!(line["actions"].as_array().is_some_and(|actions| actions.iter().any(|action| action == "set_value")), "{line}");
        }
        let reads = Cell::new(0);
        let read = || {
            reads.set(reads.get() + 1);
            Some("hello".to_string())
        };
        let Seen::Shown { line, matched, .. } = describe(&edit(Some(false)), read, 0, frame, &[frame], Some("hell")) else { panic!("the edit is not shown") };
        assert_eq!(reads.get(), 1);
        assert!(matched, "{line}");
        assert_eq!(line["value"], "hello", "{line}");
        assert!(!line["states"].as_array().is_some_and(|states| states.iter().any(|state| state == "password")), "{line}");
        // Without a Value pattern there is no password to judge and no value to read.
        let reads = Cell::new(0);
        let read = || {
            reads.set(reads.get() + 1);
            None
        };
        let button = Cached { control: Some(UIA_CONTROLTYPE_ID(50000)), value_read_only: None, password: None, invoke: true, ..edit(None) };
        let Seen::Shown { line, .. } = describe(&button, read, 0, frame, &[frame], None) else { panic!("the button is not shown") };
        assert_eq!(reads.get(), 0);
        assert!(line.get("value").is_none(), "{line}");
        assert!(!line["states"].as_array().is_some_and(|states| states.iter().any(|state| state == "password")), "{line}");
    }

    /// `get_ui_tree` takes what a walk has reached when its time is up, and a walk left behind
    /// goes on into an empty buffer: counting the children it leaves out of a line taken away
    /// with the rest must not panic (on the worker, which would then not leave COM).
    #[test]
    fn a_walk_whose_lines_were_taken_goes_on_without_them() {
        let mut arena = Arena::new();
        for number in 1..=5 {
            arena.add(0, &format!("a{number}"));
        }
        let list = arena.add(0, "List");
        for number in 1..=70 {
            arena.add(list, &format!("b{number}"));
        }
        let arena = Rc::new(arena);
        let walked = Mutex::new(Walked::default());
        // Taken when the walk reaches the 61st child of List (line 5), after its 60 shown ones.
        let take = || {
            let mut reached = walked.lock().unwrap();
            if reached.elements.len() == 66 {
                let _ = std::mem::take(&mut *reached);
            }
            None
        };
        let ended = walk_tree(vec![Root::ChildrenOf(Fake(Rc::clone(&arena), 0))], seen, |node: &Fake| node.0.labels[node.1].clone(), &walked, take);
        assert!(matches!(ended, Ended::Whole));
        let rest = walked.into_inner().unwrap();
        assert!(rest.elements.is_empty(), "{:?}", rest.elements);
        assert_eq!(rest.omitted_children, 0);
    }

    #[test]
    fn omitted_children_of_an_unnamed_top_level_container_belong_to_the_tree() {
        let mut arena = Arena::new();
        let pane = arena.add(0, "");
        for number in 1..=61 {
            arena.add(pane, &format!("t{number}"));
        }
        let (_, walked) = run(arena, &[]);
        assert_eq!(walked.elements.len(), 60);
        assert_eq!(walked.omitted_children, 1);
    }

    #[test]
    fn the_walk_stops_thirty_levels_down_and_says_how_many_children_it_left_out() {
        let mut arena = Arena::new();
        let mut parent = 0;
        for level in 0..35 {
            parent = arena.add(parent, &format!("level {level}"));
        }
        // Node 29 is "level 28": this is a sibling of "level 29", without children.
        arena.add(29, "sibling of level 29");
        let (_, walked) = run(arena, &[]);
        let lines = outline(&walked);
        assert_eq!(lines.len(), 31, "{lines:?}");
        assert_eq!(lines[29], line(29, "level 29", None, Some(1)));
        assert_eq!(lines[30], line(29, "sibling of level 29", None, None));
        assert_eq!(walked.truncated, ["depth"]);
    }

    #[test]
    fn the_walk_stops_at_five_hundred_lines() {
        let mut arena = Arena::new();
        for group in 1..=9 {
            let parent = arena.add(0, &format!("Group {group}"));
            for number in 1..=60 {
                arena.add(parent, &format!("b{group}-{number}"));
            }
        }
        let (ended, walked) = run(arena, &[]);
        assert!(matches!(ended, Ended::Whole));
        assert_eq!(walked.elements.len(), 500);
        assert_eq!(walked.truncated, ["nodes"]);
        // Exactly 500 lines is not cut short.
        let mut arena = Arena::new();
        for number in 1..=50 {
            let parent = arena.add(0, &format!("G{number}"));
            for item in 1..=9 {
                arena.add(parent, &format!("b{number}-{item}"));
            }
        }
        let (_, walked) = run(arena, &[]);
        assert_eq!(walked.elements.len(), 500);
        assert!(walked.truncated.is_empty(), "{:?}", walked.truncated);
    }

    #[test]
    fn popups_follow_the_window_from_depth_zero_without_their_siblings() {
        let mut arena = Arena::new();
        arena.add(0, "b-window");
        // The popups' parent stands in for the desktop; it is under the window only to keep one
        // arena, so the window's own walk skips it.
        let desktop = arena.add(0, "skip desktop");
        let menu = arena.add(desktop, "");
        arena.add(menu, "b-item 1");
        arena.add(menu, "b-item 2");
        arena.add(desktop, "b-another app");
        let (_, walked) = run(arena, &[menu]);
        assert_eq!(outline(&walked), [line(0, "b-window", Some(1), None), line(0, "b-item 1", Some(2), None), line(0, "b-item 2", Some(3), None)]);
    }

    #[test]
    fn a_stop_ends_the_walk() {
        let mut arena = Arena::new();
        arena.add(0, "b1");
        let walked = Mutex::new(Walked::default());
        let ended = walk_tree(vec![Root::ChildrenOf(Fake(Rc::new(arena), 0))], seen, |_: &Fake| String::new(), &walked, || Some(Ended::Stopped));
        assert!(matches!(ended, Ended::Stopped));
        assert!(walked.into_inner().unwrap().elements.is_empty());
    }

    /// m5: a debug build checks that element references are released on the worker only. Without
    /// UI Automation or a desktop element there is nothing to release, so the test says so and
    /// stops (a `should_panic` test cannot stop early, hence `catch_unwind`).
    #[cfg(debug_assertions)]
    #[test]
    fn an_element_released_off_the_worker_panics_in_a_debug_build() {
        use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
        use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let Ok(automation) = (unsafe { CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }) else {
            eprintln!("SKIP interactive: UI Automation is not available");
            return;
        };
        let Ok(desktop) = (unsafe { automation.GetRootElement() }) else {
            eprintln!("SKIP interactive: no desktop element");
            return;
        };
        let element = super::Element(desktop);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(element))).expect_err("releasing an element off the worker did not panic");
        let message = panicked.downcast_ref::<String>().map(String::as_str).or_else(|| panicked.downcast_ref::<&str>().copied()).unwrap_or("");
        assert!(message.contains("released off the UI Automation worker"), "{message}");
    }

    /// Review minor 1: a SetValue failure quotes the provider's error text only for a field that is
    /// not a password field; disabled and gone elements keep their own answers.
    #[test]
    fn a_set_value_failure_on_a_password_field_does_not_quote_the_providers_text() {
        use windows::core::{Error, HRESULT};
        let provider = Error::new(HRESULT(0x8000_4005_u32 as i32), "provider says CUMCP-secret-42");
        let failure = super::set_value_failure(&provider, true);
        assert_eq!(failure.code, "input_failed");
        assert!(!failure.message.contains("CUMCP-secret-42") && !failure.message.contains("provider says"), "{}", failure.message);
        assert!(failure.message.contains("password field"), "{}", failure.message);
        let failure = super::set_value_failure(&provider, false);
        assert!(failure.message.contains("provider says"), "{}", failure.message);
        let disabled = super::set_value_failure(&Error::new(HRESULT(0x8004_0200_u32 as i32), "x"), true);
        assert_eq!((disabled.code.as_str(), disabled.message.as_str()), ("not_supported", super::DISABLED));
        let gone = super::set_value_failure(&Error::new(HRESULT(0x8004_0201_u32 as i32), "x"), true);
        assert_eq!(gone.code, "element_not_found");
    }

    /// Ticket 65: a read-back waits a little for the state an action should leave (a provider may
    /// act after its call returns), and otherwise answers what it reads then.
    #[test]
    fn a_read_back_waits_briefly_for_the_expected_state() {
        use std::cell::Cell;
        use std::time::{Duration, Instant};
        let far = Instant::now() + Duration::from_secs(60);
        let reads = Cell::new(0);
        let late = || {
            reads.set(reads.get() + 1);
            Some(if reads.get() < 3 { "off" } else { "on" })
        };
        assert_eq!(super::read_back(late, |state| state == "on", super::READ_BACK_WAIT, far).ok(), Some(Some("on")));
        assert_eq!(reads.get(), 3);
        let started = Instant::now();
        assert_eq!(super::read_back(|| Some("collapsed"), |state| state == "expanded", super::READ_BACK_WAIT, far).ok(), Some(Some("collapsed")));
        assert!(started.elapsed() >= Duration::from_millis(300) && started.elapsed() < Duration::from_millis(1000), "{:?}", started.elapsed());
        assert_eq!(super::read_back(|| None, |_| true, super::READ_BACK_WAIT, far).ok(), Some(None));
    }

    /// Review minor 2: the wait ends `READ_BACK_MARGIN` before the job's deadline when that is
    /// sooner, and reads once even when that time is already past.
    #[test]
    fn a_read_back_stops_waiting_before_its_jobs_deadline() {
        use std::cell::Cell;
        use std::time::{Duration, Instant};
        let started = Instant::now();
        let near = started + super::READ_BACK_MARGIN + Duration::from_millis(100);
        assert_eq!(super::read_back(|| Some("off"), |state| state == "on", Duration::from_secs(5), near).ok(), Some(Some("off")));
        assert!(started.elapsed() >= Duration::from_millis(100) && started.elapsed() < Duration::from_millis(600), "{:?}", started.elapsed());
        let reads = Cell::new(0);
        let read = || {
            reads.set(reads.get() + 1);
            Some("off")
        };
        assert_eq!(super::read_back(read, |state| state == "on", Duration::from_secs(5), Instant::now()).ok(), Some(Some("off")));
        assert_eq!(reads.get(), 1);
    }

    /// Ticket 66: `settle` takes the interval it reads at and the margin it keeps before the
    /// deadline, so one loop serves both the 30 ms read-backs and the 50 ms web content wait.
    #[test]
    fn settle_reads_at_its_interval_for_its_wait_and_keeps_its_margin() {
        use std::cell::Cell;
        use std::time::{Duration, Instant};
        let far = Instant::now() + Duration::from_secs(60);
        let reads = Cell::new(0);
        let never = || {
            reads.set(reads.get() + 1);
            Some(false)
        };
        let started = Instant::now();
        let last = super::settle(never, |done| *done, Duration::from_millis(500), Duration::from_millis(50), Duration::from_millis(100), far);
        assert_eq!(last.ok(), Some(Some(false)));
        assert!(started.elapsed() >= Duration::from_millis(500) && started.elapsed() < Duration::from_millis(800), "{:?}", started.elapsed());
        // One read at once, then one every 50 ms until 500 ms: 11 reads, give or take a late wake-up.
        assert!((8..=11).contains(&reads.get()), "{} reads", reads.get());

        // Done at the third read: no more sleeping, and the value is the one that satisfied `done`.
        reads.set(0);
        let third = || {
            reads.set(reads.get() + 1);
            Some(reads.get())
        };
        let started = Instant::now();
        let ready = super::settle(third, |count| *count == 3, Duration::from_secs(5), Duration::from_millis(50), Duration::ZERO, far);
        assert_eq!(ready.ok(), Some(Some(3)));
        assert!(started.elapsed() >= Duration::from_millis(100) && started.elapsed() < Duration::from_millis(400), "{:?}", started.elapsed());

        // The margin: a deadline 600 ms away with a 500 ms margin ends the wait after about 100 ms, not 5 s.
        let started = Instant::now();
        let near = started + Duration::from_millis(600);
        let last = super::settle(|| Some(false), |done| *done, Duration::from_secs(5), Duration::from_millis(50), Duration::from_millis(500), near);
        assert_eq!(last.ok(), Some(Some(false)));
        assert!(started.elapsed() >= Duration::from_millis(100) && started.elapsed() < Duration::from_millis(400), "{:?}", started.elapsed());
    }

    /// Ticket 66: the web content wait. Content that is there from the start causes no wait;
    /// content that appears ends it; content that never does ends it after 500 ms as empty, read
    /// every 50 ms; and the wait never reaches into the time the walk needs.
    #[test]
    fn the_web_content_wait_ends_when_content_appears_and_gives_up_after_half_a_second() {
        use std::cell::Cell;
        use std::time::{Duration, Instant};
        let far = Instant::now() + Duration::from_secs(3);
        let started = Instant::now();
        assert!(!super::wait_for_web_content(|| true, far).ok().expect("not stopped"));
        assert!(started.elapsed() < Duration::from_millis(40), "content from the start must not wait: {:?}", started.elapsed());

        let reads = Cell::new(0);
        let started = Instant::now();
        let appears = || {
            reads.set(reads.get() + 1);
            reads.get() >= 4
        };
        assert!(!super::wait_for_web_content(appears, far).ok().expect("not stopped"));
        assert_eq!(reads.get(), 4);
        assert!(started.elapsed() >= Duration::from_millis(150) && started.elapsed() < Duration::from_millis(400), "{:?}", started.elapsed());

        reads.set(0);
        let started = Instant::now();
        let never = || {
            reads.set(reads.get() + 1);
            false
        };
        assert!(super::wait_for_web_content(never, far).ok().expect("not stopped"), "content that never comes is empty");
        assert!(started.elapsed() >= Duration::from_millis(500) && started.elapsed() < Duration::from_millis(800), "{:?}", started.elapsed());
        assert!((8..=11).contains(&reads.get()), "{} reads", reads.get());

        // A job that started late: 1.6 s to its deadline leaves 100 ms before the margin.
        let started = Instant::now();
        assert!(super::wait_for_web_content(|| false, started + Duration::from_millis(1600)).ok().expect("not stopped"));
        assert!(started.elapsed() < Duration::from_millis(350), "{:?}", started.elapsed());
    }

    /// Ticket 71: the web content is ready when the Document with the largest area has children;
    /// one with the most pixels decides, not any that has some.
    #[test]
    fn the_main_web_content_is_ready_when_the_largest_document_has_children() {
        use super::{area_of, main_content_ready};
        assert!(!main_content_ready(&[]), "no Document is not ready");
        assert!(main_content_ready(&[(1000, true)]));
        assert!(!main_content_ready(&[(1000, false)]));
        assert!(!main_content_ready(&[(100, true), (1000, false)]), "a side bar with content is not the main content");
        assert!(main_content_ready(&[(100, false), (1000, true)]));
        assert!(main_content_ready(&[(1000, true), (100, false)]), "whatever the order");
        assert!(main_content_ready(&[(1000, false), (1000, true)]), "of the same size, one with children is enough");
        assert!(!main_content_ready(&[(1000, false), (1000, false), (10, true)]));

        let rect = |left, top, right, bottom| Some(windows::Win32::Foundation::RECT { left, top, right, bottom });
        assert_eq!(area_of(rect(10, 20, 110, 70)), 5000);
        assert_eq!(area_of(rect(0, 0, 0, 50)), 0, "an empty rectangle");
        assert_eq!(area_of(rect(10, 10, 5, 5)), 0, "a rectangle turned inside out");
        assert_eq!(area_of(rect(0, 0, 100_000, 100_000)), 10_000_000_000, "no overflow");
        assert_eq!(area_of(None), 0, "an unreadable rectangle is the smallest, so it only decides when it is alone");
        assert!(main_content_ready(&[(area_of(None), true)]), "the only Document, unreadable, decides as before");
        assert!(!main_content_ready(&[(area_of(None), true), (1000, false)]));
    }

    /// Ticket 71: a window found empty is remembered with its title; another title, or another
    /// window, is not known; a new title replaces the old; gone windows and the oldest go.
    #[test]
    fn a_window_found_empty_is_known_by_handle_and_title_with_a_bounded_memory() {
        use super::{EmptyWaits, EMPTY_WAITS_LIMIT};
        let open = |_: isize| true;
        let mut waits = EmptyWaits::new();
        assert!(!waits.knows(1, "page"));
        waits.remember(1, "page", open);
        assert!(waits.knows(1, "page"));
        assert!(!waits.knows(1, "other page"), "a changed title waits again");
        assert!(!waits.knows(2, "page"), "another window");
        waits.remember(1, "other page", open);
        assert!(waits.knows(1, "other page") && !waits.knows(1, "page"), "one title per window");
        assert_eq!(waits.windows.len(), 1);
        waits.remember(2, "page", open);
        waits.forget(1);
        assert!(!waits.knows(1, "other page") && waits.knows(2, "page"));

        // Windows that are gone are dropped when the next is remembered.
        waits.remember(3, "page", |window| window != 2);
        assert!(!waits.knows(2, "page") && waits.knows(3, "page"));

        // The oldest go first.
        let mut waits = EmptyWaits::new();
        for window in 0..(EMPTY_WAITS_LIMIT as isize + 5) {
            waits.remember(window, "page", open);
        }
        assert_eq!(waits.windows.len(), EMPTY_WAITS_LIMIT);
        assert!(!waits.knows(0, "page") && !waits.knows(4, "page"));
        assert!(waits.knows(5, "page") && waits.knows(EMPTY_WAITS_LIMIT as isize + 4, "page"));
    }

    /// Ticket 66: which windows the wait applies to: the target's class or a child window's is
    /// `Chrome_WidgetWin_1`; any other window is read as before.
    #[test]
    fn only_a_window_of_class_chrome_widgetwin_1_or_with_a_child_of_it_is_chromium() {
        use super::is_chromium;
        let classes = |names: &[&str]| names.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        // Chrome, Edge, VS Code: the top window.
        assert!(is_chromium("Chrome_WidgetWin_1", &classes(&["Intermediate D3D Window"])));
        // A web view inside another app's window: a child.
        assert!(is_chromium("MyAppWindow", &classes(&["Edit", "Chrome_WidgetWin_1"])));
        assert!(is_chromium("chrome_widgetwin_1", &[]), "class names compare without case");
        // Others, including the neighbours of the name.
        assert!(!is_chromium("Notepad", &classes(&["Edit", "RichEditD2DPT"])));
        assert!(!is_chromium("Chrome_WidgetWin_0", &classes(&["Chrome_RenderWidgetHostHWND", "Chrome_WidgetWin_10"])));
        assert!(!is_chromium("", &[]));
    }

    /// Review minor 1: a job reaches its action only while its caller still waits for it; one the
    /// caller gave up on (abandoned, or past its deadline) is refused, having marked `Act` first.
    #[test]
    fn a_job_its_caller_gave_up_on_does_not_begin_its_action() {
        use super::{Step, Waiting};
        use std::time::{Duration, Instant};
        let waited = Step::new(Instant::now() + Duration::from_secs(60));
        assert_eq!(waited.now(), Waiting::Prepare);
        assert!(waited.begin_action().is_ok());
        assert_eq!(waited.now(), Waiting::Act);

        let abandoned = Step::new(Instant::now() + Duration::from_secs(60));
        abandoned.abandon();
        assert_eq!(abandoned.begin_action().err().map(|failure| failure.code), Some("input_failed".to_string()));

        // Past the deadline is the Prepare timeout, not a stop: no Esc was pressed (re-review minor).
        let late = Step::new(Instant::now());
        let refused = late.begin_action().err().expect("refused past its deadline");
        assert_eq!(refused.code, "input_failed");
        assert_eq!(refused.message, super::action_timeout(Waiting::Prepare).message);
        assert_eq!(late.now(), Waiting::Act);

        let prepare = super::action_timeout(Waiting::Prepare).message;
        assert!(prepare.contains("was not done") && !prepare.contains("may still perform"), "{prepare}");
    }

    /// Review minor 3: a stop after the action says it was taken and is not to be repeated.
    #[test]
    fn a_stop_after_the_action_says_it_was_taken() {
        let stopped = super::stopped_after_action();
        assert_eq!(stopped.code, "interrupted");
        assert!(stopped.message.contains("after the action was taken") && stopped.message.contains("Do not repeat the action"), "{}", stopped.message);
    }

    /// Review minor 4: one list of element actions for the tree and `element_action`; the tree
    /// lists expand and collapse by the element's state, `element_action` takes both.
    #[test]
    fn the_tree_and_element_action_list_actions_from_one_vocabulary() {
        use super::Abilities;
        use windows::Win32::UI::Accessibility::{ExpandCollapseState_Collapsed, ExpandCollapseState_Expanded, ExpandCollapseState_PartiallyExpanded};
        let all = Abilities { invoke: true, toggle: true, expand: Some(ExpandCollapseState_Collapsed), select: true, scroll_item: true, focusable: true };
        assert_eq!(all.actions(false), ["invoke", "toggle", "expand", "collapse", "select", "scroll_into_view", "focus"]);
        assert_eq!(all.actions(true), ["invoke", "toggle", "expand", "select", "scroll_into_view", "focus"]);
        let open = Abilities { invoke: false, toggle: false, expand: Some(ExpandCollapseState_Expanded), select: false, scroll_item: false, focusable: false };
        assert_eq!(open.actions(true), ["collapse"]);
        assert_eq!(open.actions(false), ["expand", "collapse"]);
        let partly = Abilities { expand: Some(ExpandCollapseState_PartiallyExpanded), ..open };
        assert_eq!(partly.actions(true), ["expand", "collapse"]);
        let none = Abilities { expand: None, ..partly };
        assert!(none.actions(false).is_empty());
    }

    #[test]
    fn long_text_is_cut_with_its_length_kept() {
        assert_eq!(cut_text("abc".to_string(), 3), ("abc".to_string(), None));
        assert_eq!(cut_text("名名名名".to_string(), 3), ("名名名".to_string(), Some(4)));
    }

    const OWNER: (u32, u32) = (100, 200);

    #[test]
    fn the_cache_keeps_the_newest_five_trees_of_each_window_and_none_of_closed_windows() {
        let mut cache: TreeCache<u32> = TreeCache::new();
        let mut dropped = Vec::new();
        for number in 1..=7 {
            dropped.extend(cache.insert(format!("t{number}"), 10, OWNER, vec![number], |_| Some(OWNER)));
            dropped.extend(cache.insert(format!("u{number}"), 20, OWNER, vec![number + 10], |_| Some(OWNER)));
        }
        assert_eq!(cache.tree_ids(10), ["t3", "t4", "t5", "t6", "t7"]);
        assert_eq!(cache.tree_ids(20), ["u3", "u4", "u5", "u6", "u7"]);
        // The dropped trees' elements are handed back, to be released on the worker.
        assert_eq!(dropped, [vec![1], vec![11], vec![2], vec![12]]);
        let closed = cache.insert("v1".to_string(), 30, OWNER, vec![], |window| (window != 20).then_some(OWNER));
        assert_eq!(closed.len(), 5);
        assert!(cache.tree_ids(20).is_empty());
        assert_eq!(cache.tree_ids(10).len(), 5);
        assert_eq!(cache.tree_ids(30), ["v1"]);
    }

    /// Ticket 62: a window handle Windows has given to a new window (of another thread) since.
    #[test]
    fn the_cache_drops_the_trees_of_a_window_handle_another_window_now_has() {
        let mut cache: TreeCache<u32> = TreeCache::new();
        cache.insert("t1".to_string(), 10, OWNER, vec![1], |_| Some(OWNER));
        cache.insert("t2".to_string(), 20, OWNER, vec![2], |_| Some(OWNER));
        let dropped = cache.insert("t3".to_string(), 20, OWNER, vec![3], |window| Some(if window == 10 { (100, 201) } else { OWNER }));
        assert_eq!(dropped, [vec![1]]);
        assert!(cache.tree_ids(10).is_empty());
    }

    fn code<T: std::fmt::Debug>(found: Result<T, crate::protocol::Failure>) -> (String, String) {
        let failure = found.expect_err("a failure");
        (failure.code, failure.message)
    }

    #[test]
    fn an_element_is_found_by_tree_window_owner_and_index() {
        let mut cache: TreeCache<&str> = TreeCache::new();
        cache.insert("t1-abcde".to_string(), 10, OWNER, vec!["edit", "button", "check"], |_| Some(OWNER));
        cache.insert("t2-abcde".to_string(), 20, OWNER, vec![], |_| Some(OWNER));
        assert_eq!(cache.find("t1-abcde", 10, Some(OWNER), 2).copied().ok(), Some("button"));

        // Not a tree of this helper (dropped, or read by another helper process): no_ui_tree.
        let (unknown, message) = code(cache.find("t9-zzzzz", 10, Some(OWNER), 1));
        assert_eq!(unknown, "no_ui_tree");
        assert!(message.contains("t9-zzzzz"), "{message}");
        // A tree of another window.
        let (other, message) = code(cache.find("t1-abcde", 20, Some(OWNER), 1));
        assert_eq!(other, "invalid_params");
        assert!(message.contains("another window"), "{message}");
        // The handle now names a window of another thread: that window is not the one the tree is of.
        let (reused, message) = code(cache.find("t1-abcde", 10, Some((100, 201)), 1));
        assert_eq!(reused, "element_not_found");
        assert!(message.contains("new tree"), "{message}");
        let (closed, _) = code(cache.find("t1-abcde", 10, None, 1));
        assert_eq!(closed, "element_not_found");
        // Past the tree, with its range, as the server words it.
        for index in [0, 4] {
            let (range, message) = code(cache.find("t1-abcde", 10, Some(OWNER), index));
            assert_eq!(range, "invalid_params");
            assert_eq!(message, format!("element_index {index} is not in UI tree t1-abcde, which has element_index 1 to 3."));
        }
        let (empty, message) = code(cache.find("t2-abcde", 20, Some(OWNER), 1));
        assert_eq!(empty, "invalid_params");
        assert!(message.contains("no numbered elements"), "{message}");
    }

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> windows::Win32::Foundation::RECT {
        windows::Win32::Foundation::RECT { left, top, right, bottom }
    }

    /// Ticket 140: a background click tries Invoke, Toggle, Select, ExpandCollapse (by state) in that
    /// order, those the element has, and nothing when it has none.
    #[test]
    fn a_click_tries_the_patterns_the_element_has_in_the_order_invoke_toggle_select_expand_collapse() {
        use super::{click_actions, Abilities, Action};
        use windows::Win32::UI::Accessibility::{ExpandCollapseState_Collapsed, ExpandCollapseState_Expanded, ExpandCollapseState_PartiallyExpanded};
        let none = || Abilities { invoke: false, toggle: false, expand: None, select: false, scroll_item: true, focusable: true };
        assert!(click_actions(&none()).is_empty(), "scroll_item and focus are not click patterns");
        let expand = |state| Abilities { expand: Some(state), ..none() };
        assert!(click_actions(&expand(ExpandCollapseState_Collapsed)) == [Action::Expand]);
        assert!(click_actions(&expand(ExpandCollapseState_PartiallyExpanded)) == [Action::Expand]);
        assert!(click_actions(&expand(ExpandCollapseState_Expanded)) == [Action::Collapse]);
        let all = || Abilities { invoke: true, toggle: true, expand: Some(ExpandCollapseState_Collapsed), select: true, ..none() };
        assert!(click_actions(&all()) == [Action::Invoke, Action::Toggle, Action::Select, Action::Expand]);
        assert!(click_actions(&Abilities { invoke: false, ..all() }) == [Action::Toggle, Action::Select, Action::Expand]);
        assert!(click_actions(&Abilities { invoke: false, toggle: false, ..all() }) == [Action::Select, Action::Expand]);
        assert!(click_actions(&Abilities { invoke: false, toggle: false, select: false, ..all() }) == [Action::Expand]);
    }

    #[test]
    fn an_element_click_aims_at_the_centre_of_its_visible_part() {
        let frame = rect(100, 100, 600, 500);
        let monitors = [rect(0, 0, 1920, 1080)];
        // Wholly visible.
        assert_eq!(aim(rect(110, 150, 230, 178), &[], &[frame], &monitors, false), Ok((170, 164)));
        // A list row whose bottom half is below its list (an ancestor): the centre of the top half.
        assert_eq!(aim(rect(200, 300, 400, 320), &[rect(190, 220, 410, 310)], &[frame], &monitors, false), Ok((300, 305)));
        // An ancestor without an area (some containers report none) does not hide its children.
        assert_eq!(aim(rect(110, 150, 230, 178), &[rect(0, 0, 0, 0)], &[frame], &monitors, false), Ok((170, 164)));
        // In the window's popup (a menu below the window), outside the window itself.
        let menu = rect(150, 480, 300, 700);
        assert_eq!(aim(rect(160, 600, 290, 620), &[menu], &[frame, menu], &monitors, false), Ok((225, 610)));
        // Half off the right edge of every screen: the centre of the part on a screen.
        let wide = [rect(0, 0, 400, 1080)];
        assert_eq!(aim(rect(300, 150, 500, 170), &[], &[frame], &wide, false), Ok((350, 160)));
        // Outside an ancestor's rectangle (a tree item under its parent item, whose rectangle is
        // its own row; a menu under the small button it drops from): ancestors only choose the
        // point, they do not hide it, so the centre of its own part in the window.
        assert_eq!(aim(rect(200, 330, 400, 350), &[rect(190, 220, 410, 310)], &[frame], &monitors, false), Ok((300, 340)));
        // Inside its ancestors only where that is outside the window: its part in the window.
        assert_eq!(aim(rect(110, 480, 230, 520), &[rect(100, 505, 300, 530)], &[frame], &monitors, false), Ok((170, 490)));
        // Ancestors clip one at a time, nearest first, and one it lies outside of is passed over:
        // a nested item outside its parent row is still clipped by the list holding them (the
        // centre of its top half, which the list shows), not clicked at its own centre.
        let row = rect(190, 220, 410, 240);
        let list = rect(190, 220, 410, 310);
        assert_eq!(aim(rect(200, 300, 400, 320), &[row, list], &[frame], &monitors, false), Ok((300, 305)));
    }

    #[test]
    fn an_element_out_of_view_or_off_every_screen_is_not_aimed_at() {
        let frame = rect(100, 100, 600, 500);
        let monitors = [rect(0, 0, 1920, 1080)];
        // Outside the window and its popups, or without an area.
        assert_eq!(aim(rect(700, 150, 800, 170), &[], &[frame], &monitors, false), Err(Missed::NotVisible));
        assert_eq!(aim(rect(200, 200, 200, 220), &[], &[frame], &monitors, false), Err(Missed::NotVisible));
        // On screen and in the window, but UI Automation says it is out of view (a row scrolled
        // out of its list, which says so).
        assert_eq!(aim(rect(110, 150, 230, 178), &[], &[frame], &monitors, true), Err(Missed::NotVisible));
        assert_eq!(aim(rect(200, 330, 400, 350), &[rect(190, 220, 410, 310)], &[frame], &monitors, true), Err(Missed::NotVisible));
        // In the window, but the window part it is in is off every screen (whatever UI Automation says).
        let off = rect(-400, 100, 100, 500);
        for flag in [false, true] {
            assert_eq!(aim(rect(-300, 150, -200, 170), &[], &[off], &monitors, flag), Err(Missed::Offscreen));
        }
    }

    #[test]
    fn the_tree_cache_request_never_carries_a_value() {
        // A cached value would bring a password field's text into this process with the rest.
        use windows::Win32::UI::Accessibility::{UIA_LegacyIAccessibleValuePropertyId, UIA_ValueValuePropertyId};
        assert!(!super::TREE_PROPERTIES.contains(&UIA_ValueValuePropertyId));
        assert!(!super::TREE_PROPERTIES.contains(&UIA_LegacyIAccessibleValuePropertyId));
    }
}

#[cfg(test)]
mod late_watch_tests {
    use super::*;

    #[test]
    fn only_an_action_whose_own_call_timed_out_keeps_its_watch() {
        assert!(keeps_watching(true, Waiting::Act));
        for waiting in [Waiting::Aim, Waiting::Prepare, Waiting::ReadBack] {
            assert!(!keeps_watching(true, waiting), "{waiting:?}");
        }
        for waiting in [Waiting::Aim, Waiting::Prepare, Waiting::Act, Waiting::ReadBack] {
            assert!(!keeps_watching(false, waiting), "an answered action ends its watch as usual: {waiting:?}");
        }
    }

    #[test]
    fn the_job_says_it_returned_however_it_ends() {
        let returned = Arc::new(AtomicBool::new(false));
        drop(SetOnDrop(returned.clone()));
        assert!(returned.load(Ordering::SeqCst));
        let panicked = Arc::new(AtomicBool::new(false));
        let flag = SetOnDrop(panicked.clone());
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _flag = flag;
            panic!("a job that panics");
        }));
        assert!(panicked.load(Ordering::SeqCst));
    }
}
