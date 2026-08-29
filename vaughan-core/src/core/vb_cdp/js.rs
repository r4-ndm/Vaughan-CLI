//! Embedded JS snippets for CDP evaluation.
//!
//! Snippets live in `js/*.js` (syntax-highlighted, reviewable) and are woven
//! into expressions here via `__VB_*__` placeholder substitution — no
//! `format!` brace escaping. Values substituted into `__VB_*_JSON__`-style
//! slots must already be JSON-encoded (quotes included).

/// Ordered interactive elements IIFE (shared by snapshot/click/type snippets).
pub(crate) const INTERACTIVE_ELS: &str = include_str!("js/interactive_els.js");
/// Deep click-by-text function expression `(needle, skipBanner) => {...}`.
pub(crate) const DEEP_CLICK_BY_TEXT: &str = include_str!("js/deep_click_by_text.js");
/// Visible body text lines IIFE.
pub(crate) const VISIBLE_LINES: &str = include_str!("js/visible_lines.js");
/// Token-picker modal presence probe IIFE.
pub(crate) const MODAL_PROBE: &str = include_str!("js/modal_probe.js");
/// Swap CTA click IIFE (no parameters).
pub(crate) const CLICK_SWAP_SUBMIT: &str = include_str!("js/click_swap_submit.js");

const SNAPSHOT_REFS: &str = include_str!("js/snapshot_refs.js");
const CLICK_REF: &str = include_str!("js/click_ref.js");
const CLICK_SNAPSHOT_REF: &str = include_str!("js/click_snapshot_ref.js");
const OPEN_TOKEN_PICKER: &str = include_str!("js/open_token_picker.js");
const SEARCH_TOKEN: &str = include_str!("js/search_token.js");
const PICK_TOKEN: &str = include_str!("js/pick_token.js");
const SET_AMOUNT: &str = include_str!("js/set_amount.js");
const TYPE_INTO_REF: &str = include_str!("js/type_into_ref.js");
const WAIT_PROBE: &str = include_str!("js/wait_probe.js");

/// Snapshot of interactive refs (`{ title, url, refs }`).
pub(crate) fn snapshot_refs() -> String {
    SNAPSHOT_REFS.replace("__VB_INTERACTIVE_ELS__", INTERACTIVE_ELS)
}

/// Click the element at snapshot index `idx`.
pub(crate) fn click_ref(idx: u32) -> String {
    CLICK_REF
        .replace("__VB_INTERACTIVE_ELS__", INTERACTIVE_ELS)
        .replace("__VB_IDX__", &idx.to_string())
}

/// Click the snapshot element whose label matches `needle_json` (JSON string).
pub(crate) fn click_snapshot_ref(needle_json: &str) -> String {
    CLICK_SNAPSHOT_REF
        .replace("__VB_INTERACTIVE_ELS__", INTERACTIVE_ELS)
        .replace("__VB_NEEDLE__", needle_json)
}

/// Open the token picker on `side`, skipping candidates labelled `avoid_json`.
pub(crate) fn open_token_picker(symbol_json: &str, side: &str, avoid_json: &str) -> String {
    OPEN_TOKEN_PICKER
        .replace("__VB_SYMBOL__", symbol_json)
        .replace("__VB_SIDE__", side)
        .replace("__VB_AVOID__", avoid_json)
}

/// Type the symbol into the picker search box.
pub(crate) fn search_token(symbol_json: &str) -> String {
    SEARCH_TOKEN.replace("__VB_SYMBOL__", symbol_json)
}

/// Click the matching token row in the (filtered) picker modal.
pub(crate) fn pick_token(symbol_json: &str) -> String {
    PICK_TOKEN.replace("__VB_SYMBOL__", symbol_json)
}

/// Set the sell amount (`amount_json` is a JSON string).
pub(crate) fn set_amount(amount_json: &str) -> String {
    SET_AMOUNT.replace("__VB_AMOUNT__", amount_json)
}

/// Focus snapshot ref `idx` and type `text_json`; `clear` replaces first.
pub(crate) fn type_into_ref(idx: u32, text_json: &str, clear: bool, typed_len: usize) -> String {
    TYPE_INTO_REF
        .replace("__VB_INTERACTIVE_ELS__", INTERACTIVE_ELS)
        .replace("__VB_IDX__", &idx.to_string())
        .replace("__VB_TEXT__", text_json)
        .replace("__VB_CLEAR__", if clear { "true" } else { "false" })
        .replace("__VB_TYPED_LEN__", &typed_len.to_string())
}

/// Wait probe for text / selector / URL substring (each a JSON literal or `null`).
pub(crate) fn wait_probe(text_lit: &str, selector_lit: &str, url_lit: &str) -> String {
    WAIT_PROBE
        .replace("__VB_TEXT__", text_lit)
        .replace("__VB_SELECTOR__", selector_lit)
        .replace("__VB_URL_PART__", url_lit)
}
