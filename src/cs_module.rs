//! C# bindings for the harmony crate.
//!
//! The bindings are kept intentionally small: we expose the `HarmonyEncoding` type
//! together with the operations that are required by the original Rust test
//! suite (rendering a conversation for completion, parsing messages from
//! completion tokens and decoding tokens back into UTF-8). All higher-level
//! data-structures (Conversation, Message, SystemContent, DeveloperContent, …) are passed across the FFI
//! boundary as JSON.  This allows us to keep the Rust ↔ C# interface very
//! light-weight while still re-using the exact same logic that is implemented
//! in Rust.
//!
//! A thin, typed, user-facing C# wrapper around these low-level bindings is
//! provided after rust compilation in `target/HarmonyBindings.cs`.
// src/cs_module.rs

#![allow(unused)]
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::ptr;
use serde_json::json;
use std::cell::RefCell;
use base64::{engine::general_purpose, Engine as _};

use crate::{
    chat::{Message, Role, ToolNamespaceConfig},
    encoding::{HarmonyEncoding, StreamableParser, RenderConversationConfig, RenderOptions},
    load_harmony_encoding, HarmonyEncodingName,
};

// --- Thread-local last error ---
thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = RefCell::new(None);
}

fn set_last_error(err: impl AsRef<str>) {
    let s = CString::new(err.as_ref()).unwrap_or_else(|_| CString::new("unknown error").unwrap());
    LAST_ERROR.with(|c| *c.borrow_mut() = Some(s));
}

// helper to convert Rust String -> *mut c_char (caller must free with harmony_free_string)
fn string_to_c(s: String) -> *mut c_char {
    CString::new(s).unwrap().into_raw()
}

// helper to read optional c string
unsafe fn opt_cstr_to_opt_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() { return None; }
    CStr::from_ptr(ptr).to_str().ok().map(|s| s.to_string())
}

// --- Expose get_last_error ---
#[no_mangle]
pub extern "C" fn harmony_get_last_error() -> *mut c_char {
    LAST_ERROR.with(|c| {
        if let Some(ref s) = *c.borrow() {
            // return a fresh allocation the caller must free
            CString::new(s.to_str().unwrap_or("")).unwrap().into_raw()
        } else {
            ptr::null_mut()
        }
    })
}

/// Free a string returned by this library.
#[no_mangle]
pub extern "C" fn harmony_free_string(s: *mut c_char) {
    if s.is_null() { return; }
    unsafe { CString::from_raw(s); }
}

// -------------------- HarmonyEncoding handle --------------------
#[no_mangle]
pub extern "C" fn harmony_encoding_new(name: *const c_char) -> *mut c_void {
    let name_opt = unsafe { opt_cstr_to_opt_string(name) };
    let name_str = match name_opt {
        Some(s) => s,
        None => {
            set_last_error("name is null or invalid");
            return ptr::null_mut();
        }
    };

    // parse as HarmonyEncodingName
    let parsed: HarmonyEncodingName = match name_str.parse() {
        Ok(v) => v,
        Err(e) => {
            set_last_error(format!("invalid encoding name: {}", e));
            return ptr::null_mut();
        }
    };

    match load_harmony_encoding(parsed) {
        Ok(enc) => {
            let boxed = Box::new(enc);
            Box::into_raw(boxed) as *mut c_void
        }
        Err(e) => {
            set_last_error(e.to_string());
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn harmony_encoding_free(handle: *mut c_void) {
    if handle.is_null() { return; }
    unsafe {
        let _boxed: Box<HarmonyEncoding> = Box::from_raw(handle as *mut HarmonyEncoding);
        // dropped here
    }
}

#[no_mangle]
pub extern "C" fn harmony_encoding_name(handle: *mut c_void) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let enc = unsafe { &*(handle as *mut HarmonyEncoding) };
    string_to_c(enc.name().to_string())
}

// All rendering functions will accept JSON strings and optional config JSON (or NULL).
// They return a JSON string that encodes the token array (e.g. "[1,2,3]") or NULL on error.

fn parse_render_config(config_json: Option<String>) -> Option<RenderConversationConfig> {
    config_json.and_then(|s| serde_json::from_str::<RenderConversationConfig>(&s).ok())
}

#[no_mangle]
pub extern "C" fn harmony_render_conversation_for_completion(
    handle: *mut c_void,
    conversation_json: *const c_char,
    next_turn_role: *const c_char,
    config_json: *const c_char, // optional JSON string or NULL
) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let enc = unsafe { &*(handle as *mut HarmonyEncoding) };

    let conversation_str = unsafe { opt_cstr_to_opt_string(conversation_json) };
    let role_str = unsafe { opt_cstr_to_opt_string(next_turn_role) };
    if conversation_str.is_none() || role_str.is_none() {
        set_last_error("conversation_json or next_turn_role is null/invalid");
        return ptr::null_mut();
    }
    let conv: crate::chat::Conversation = match serde_json::from_str(&conversation_str.unwrap()) {
        Ok(c) => c,
        Err(e) => {
            set_last_error(format!("invalid conversation JSON: {}", e));
            return ptr::null_mut();
        }
    };
    let role = match Role::try_from(&role_str.unwrap()[..]) {
        Ok(r) => r,
        Err(_) => {
            set_last_error("unknown role");
            return ptr::null_mut();
        }
    };
    let config_opt = unsafe { opt_cstr_to_opt_string(config_json) };
    let rust_config = parse_render_config(config_opt);

    match enc.render_conversation_for_completion(&conv, role, rust_config.as_ref()) {
        Ok(tokens) => {
            // serialize Vec<u32> as JSON string
            match serde_json::to_string(&tokens) {
                Ok(s) => string_to_c(s),
                Err(e) => {
                    set_last_error(format!("serialisation error: {}", e));
                    ptr::null_mut()
                }
            }
        }
        Err(e) => {
            set_last_error(e.to_string());
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn harmony_render_conversation(
    handle: *mut c_void,
    conversation_json: *const c_char,
    config_json: *const c_char,
) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let enc = unsafe { &*(handle as *mut HarmonyEncoding) };

    let conversation_str = unsafe { opt_cstr_to_opt_string(conversation_json) };
    if conversation_str.is_none() {
        set_last_error("conversation_json is null/invalid");
        return ptr::null_mut();
    }
    let conv: crate::chat::Conversation = match serde_json::from_str(&conversation_str.unwrap()) {
        Ok(c) => c,
        Err(e) => {
            set_last_error(format!("invalid conversation JSON: {}", e));
            return ptr::null_mut();
        }
    };
    let config_opt = unsafe { opt_cstr_to_opt_string(config_json) };
    let rust_config = parse_render_config(config_opt);

    match enc.render_conversation(&conv, rust_config.as_ref()) {
        Ok(tokens) => serde_json::to_string(&tokens).map(|s| string_to_c(s)).unwrap_or_else(|e| {
            set_last_error(format!("serialisation error: {}", e));
            ptr::null_mut()
        }),
        Err(e) => {
            set_last_error(e.to_string());
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn harmony_render_conversation_for_training(
    handle: *mut c_void,
    conversation_json: *const c_char,
    config_json: *const c_char,
) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let enc = unsafe { &*(handle as *mut HarmonyEncoding) };

    let conversation_str = unsafe { opt_cstr_to_opt_string(conversation_json) };
    if conversation_str.is_none() {
        set_last_error("conversation_json is null/invalid");
        return ptr::null_mut();
    }
    let conv: crate::chat::Conversation = match serde_json::from_str(&conversation_str.unwrap()) {
        Ok(c) => c,
        Err(e) => {
            set_last_error(format!("invalid conversation JSON: {}", e));
            return ptr::null_mut();
        }
    };
    let config_opt = unsafe { opt_cstr_to_opt_string(config_json) };
    let rust_config = parse_render_config(config_opt);

    match enc.render_conversation_for_training(&conv, rust_config.as_ref()) {
        Ok(tokens) => serde_json::to_string(&tokens).map(|s| string_to_c(s)).unwrap_or_else(|e| {
            set_last_error(format!("serialisation error: {}", e));
            ptr::null_mut()
        }),
        Err(e) => {
            set_last_error(e.to_string());
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn harmony_render(
    handle: *mut c_void,
    message_json: *const c_char,
    render_options_json: *const c_char, // optional
) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let enc = unsafe { &*(handle as *mut HarmonyEncoding) };

    let message_str = unsafe { opt_cstr_to_opt_string(message_json) };
    if message_str.is_none() {
        set_last_error("message_json is null/invalid");
        return ptr::null_mut();
    }
    let msg: crate::chat::Message = match serde_json::from_str(&message_str.unwrap()) {
        Ok(m) => m,
        Err(e) => {
            set_last_error(format!("invalid message JSON: {}", e));
            return ptr::null_mut();
        }
    };

    let rust_options = unsafe { opt_cstr_to_opt_string(render_options_json) }
        .and_then(|s| serde_json::from_str::<RenderOptions>(&s).ok());

    match enc.render(&msg, rust_options.as_ref()) {
        Ok(tokens) => serde_json::to_string(&tokens).map(|s| string_to_c(s)).unwrap_or_else(|e| {
            set_last_error(format!("serialisation error: {}", e));
            ptr::null_mut()
        }),
        Err(e) => {
            set_last_error(e.to_string());
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn harmony_parse_messages_from_completion_tokens(
    handle: *mut c_void,
    tokens_json: *const c_char, // expect JSON array e.g. "[1,2,3]"
    role: *const c_char,        // optional
) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let enc = unsafe { &*(handle as *mut HarmonyEncoding) };

    let tokens_str = unsafe { opt_cstr_to_opt_string(tokens_json) };
    if tokens_str.is_none() {
        set_last_error("tokens_json is null/invalid");
        return ptr::null_mut();
    }
    let tokens: Vec<u32> = match serde_json::from_str(&tokens_str.unwrap()) {
        Ok(v) => v,
        Err(e) => {
            set_last_error(format!("invalid tokens JSON: {}", e));
            return ptr::null_mut();
        }
    };

    let role_parsed = unsafe { opt_cstr_to_opt_string(role) }
        .map(|r| Role::try_from(r.as_str()))
        .transpose()
        .map_err(|_| ())
        .ok()
        .flatten();

    let messages: Vec<crate::chat::Message> = match enc.parse_messages_from_completion_tokens(tokens, role_parsed) {
        Ok(m) => m,
        Err(e) => {
            set_last_error(e.to_string());
            return ptr::null_mut();
        }
    };

    match serde_json::to_string(&messages) {
        Ok(s) => string_to_c(s),
        Err(e) => {
            set_last_error(format!("serialisation error: {}", e));
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn harmony_decode_utf8(
    handle: *mut c_void,
    tokens_json: *const c_char,
) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let enc = unsafe { &*(handle as *mut HarmonyEncoding) };

    let tokens_str = unsafe { opt_cstr_to_opt_string(tokens_json) };
    if tokens_str.is_none() {
        set_last_error("tokens_json is null/invalid");
        return ptr::null_mut();
    }
    let tokens: Vec<u32> = match serde_json::from_str(&tokens_str.unwrap()) {
        Ok(v) => v,
        Err(e) => {
            set_last_error(format!("invalid tokens JSON: {}", e));
            return ptr::null_mut();
        }
    };

    match enc.tokenizer().decode_utf8(tokens) {
        Ok(s) => string_to_c(s),
        Err(e) => {
            set_last_error(e.to_string());
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn harmony_decode_bytes(
    handle: *mut c_void,
    tokens_json: *const c_char,
) -> *mut c_char {
    // returns base64 string of bytes
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let enc = unsafe { &*(handle as *mut HarmonyEncoding) };

    let tokens_str = unsafe { opt_cstr_to_opt_string(tokens_json) };
    if tokens_str.is_none() {
        set_last_error("tokens_json is null/invalid");
        return ptr::null_mut();
    }
    let tokens: Vec<u32> = match serde_json::from_str(&tokens_str.unwrap()) {
        Ok(v) => v,
        Err(e) => {
            set_last_error(format!("invalid tokens JSON: {}", e));
            return ptr::null_mut();
        }
    };

    match enc.tokenizer().decode_bytes(tokens) {
        Ok(bytes) => {
            let b64 = general_purpose::STANDARD.encode(&bytes);
            string_to_c(b64)
        }
        Err(e) => {
            set_last_error(e.to_string());
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn harmony_encode(
    handle: *mut c_void,
    text: *const c_char,
    allowed_special_json: *const c_char, // optional JSON array of strings
) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let enc = unsafe { &*(handle as *mut HarmonyEncoding) };

    let text_str = unsafe { opt_cstr_to_opt_string(text) }.unwrap_or_default();
    let allowed_opt = unsafe { opt_cstr_to_opt_string(allowed_special_json) };
    let allowed_set = allowed_opt
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .map(|v| v.into_iter().collect::<std::collections::HashSet<String>>())
        .unwrap_or_default();
    let allowed_refset: std::collections::HashSet<&str> =
        allowed_set.iter().map(|s| s.as_str()).collect();

    let (tokens, _extra) = enc.tokenizer().encode(&text_str, &allowed_refset);
    serde_json::to_string(&tokens).map(|s| string_to_c(s)).unwrap_or_else(|e| {
        set_last_error(format!("serialisation error: {}", e));
        ptr::null_mut()
    })
}

#[no_mangle]
pub extern "C" fn harmony_special_tokens(handle: *mut c_void) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let enc = unsafe { &*(handle as *mut HarmonyEncoding) };
    let toks: Vec<String> = enc.tokenizer().special_tokens().into_iter().map(str::to_string).collect();
    serde_json::to_string(&toks).map(|s| string_to_c(s)).unwrap_or_else(|e| {
        set_last_error(format!("serialisation error: {}", e));
        ptr::null_mut()
    })
}

#[no_mangle]
pub extern "C" fn harmony_is_special_token(handle: *mut c_void, token: u32) -> i32 {
    if handle.is_null() {
        set_last_error("null handle");
        return -1;
    }
    let enc = unsafe { &*(handle as *mut HarmonyEncoding) };
    if enc.tokenizer().is_special_token(token) { 1 } else { 0 }
}

#[no_mangle]
pub extern "C" fn harmony_stop_tokens(handle: *mut c_void) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let enc = unsafe { &*(handle as *mut HarmonyEncoding) };
    match enc.stop_tokens() {
        Ok(set) => {
            let vec: Vec<u32> = set.into_iter().collect();
            serde_json::to_string(&vec).map(|s| string_to_c(s)).unwrap_or_else(|e| {
                set_last_error(format!("serialisation error: {}", e));
                ptr::null_mut()
            })
        }
        Err(e) => {
            set_last_error(e.to_string());
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn harmony_stop_tokens_for_assistant_actions(handle: *mut c_void) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let enc = unsafe { &*(handle as *mut HarmonyEncoding) };
    match enc.stop_tokens_for_assistant_actions() {
        Ok(set) => {
            let vec: Vec<u32> = set.into_iter().collect();
            serde_json::to_string(&vec).map(|s| string_to_c(s)).unwrap_or_else(|e| {
                set_last_error(format!("serialisation error: {}", e));
                ptr::null_mut()
            })
        }
        Err(e) => {
            set_last_error(e.to_string());
            ptr::null_mut()
        }
    }
}

// -------------------- StreamableParser handle --------------------
#[no_mangle]
pub extern "C" fn harmony_streamable_parser_new(
    encoding_handle: *mut c_void,
    role: *const c_char, // optional
) -> *mut c_void {
    if encoding_handle.is_null() {
        set_last_error("null encoding handle");
        return ptr::null_mut();
    }
    let enc = unsafe { &*(encoding_handle as *mut HarmonyEncoding) };
    let role_parsed = unsafe { opt_cstr_to_opt_string(role) }
        .map(|r| Role::try_from(r.as_str()))
        .transpose()
        .map_err(|_| ())
        .ok()
        .flatten();

    match StreamableParser::new(enc.clone(), role_parsed) {
        Ok(parser) => Box::into_raw(Box::new(parser)) as *mut c_void,
        Err(e) => {
            set_last_error(e.to_string());
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn harmony_streamable_parser_free(handle: *mut c_void) {
    if handle.is_null() { return; }
    unsafe { let _boxed: Box<StreamableParser> = Box::from_raw(handle as *mut StreamableParser); }
}

#[no_mangle]
pub extern "C" fn harmony_streamable_parser_process(handle: *mut c_void, token: u32) -> i32 {
    if handle.is_null() {
        set_last_error("null handle");
        return -1;
    }
    let parser = unsafe { &mut *(handle as *mut StreamableParser) };
    match parser.process(token) {
        Ok(_) => 0,
        Err(e) => { set_last_error(e.to_string()); -1 }
    }
}

#[no_mangle]
pub extern "C" fn harmony_streamable_parser_process_eos(handle: *mut c_void) -> i32 {
    if handle.is_null() {
        set_last_error("null handle");
        return -1;
    }
    let parser = unsafe { &mut *(handle as *mut StreamableParser) };
    match parser.process_eos() {
        Ok(_) => 0,
        Err(e) => { set_last_error(e.to_string()); -1 }
    }
}

#[no_mangle]
pub extern "C" fn harmony_streamable_parser_current_content(handle: *mut c_void) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let parser = unsafe { &*(handle as *mut StreamableParser) };
    match parser.current_content() {
        Ok(s) => string_to_c(s),
        Err(e) => { set_last_error(e.to_string()); ptr::null_mut() }
    }
}

#[no_mangle]
pub extern "C" fn harmony_streamable_parser_current_role(handle: *mut c_void) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let parser = unsafe { &*(handle as *mut StreamableParser) };
    parser.current_role().map(|r| CString::new(r.as_str()).unwrap().into_raw()).unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn harmony_streamable_parser_current_content_type(handle: *mut c_void) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let parser = unsafe { &*(handle as *mut StreamableParser) };
    parser.current_content_type().map(|s| CString::new(s).unwrap().into_raw()).unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn harmony_streamable_parser_last_content_delta(handle: *mut c_void) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let parser = unsafe { &*(handle as *mut StreamableParser) };
    match parser.last_content_delta() {
        Ok(opt) => {
            match opt {
                Some(s) => CString::new(s).unwrap().into_raw(),
                None => ptr::null_mut()
            }
        }
        Err(e) => { set_last_error(e.to_string()); ptr::null_mut() }
    }
}

#[no_mangle]
pub extern "C" fn harmony_streamable_parser_messages(handle: *mut c_void) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let parser = unsafe { &*(handle as *mut StreamableParser) };
    match serde_json::to_string(parser.messages()) {
        Ok(s) => string_to_c(s),
        Err(e) => { set_last_error(e.to_string()); ptr::null_mut() }
    }
}

#[no_mangle]
pub extern "C" fn harmony_streamable_parser_tokens(handle: *mut c_void) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let parser = unsafe { &*(handle as *mut StreamableParser) };
    let v = parser.tokens().to_vec();
    match serde_json::to_string(&v) {
        Ok(s) => string_to_c(s),
        Err(e) => { set_last_error(e.to_string()); ptr::null_mut() }
    }
}

#[no_mangle]
pub extern "C" fn harmony_streamable_parser_state(handle: *mut c_void) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let parser = unsafe { &*(handle as *mut StreamableParser) };
    match parser.state_json() {
        Ok(s) => string_to_c(s),
        Err(e) => { set_last_error(e.to_string()); ptr::null_mut() }
    }
}

#[no_mangle]
pub extern "C" fn harmony_streamable_parser_current_recipient(handle: *mut c_void) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let parser = unsafe { &*(handle as *mut StreamableParser) };
    parser.current_recipient().map(|s| CString::new(s).unwrap().into_raw()).unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn harmony_streamable_parser_current_channel(handle: *mut c_void) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    let parser = unsafe { &*(handle as *mut StreamableParser) };
    parser.current_channel().map(|s| CString::new(s).unwrap().into_raw()).unwrap_or(ptr::null_mut())
}

// -------------------- Utility: get_tool_namespace_config --------------------
#[no_mangle]
pub extern "C" fn harmony_get_tool_namespace_config(tool: *const c_char) -> *mut c_char {
    let tool_str = unsafe { opt_cstr_to_opt_string(tool) };
    let t = match tool_str {
        Some(s) => s,
        None => {
            set_last_error("tool is null/invalid");
            return ptr::null_mut();
        }
    };

    let cfg = match t.as_str() {
        "browser" => ToolNamespaceConfig::browser(),
        "python" => ToolNamespaceConfig::python(),
        _ => {
            set_last_error("unknown tool namespace");
            return ptr::null_mut();
        }
    };

    match serde_json::to_string(&serde_json::to_value(&cfg).unwrap()) {
        Ok(s) => string_to_c(s),
        Err(e) => { set_last_error(e.to_string()); ptr::null_mut() }
    }
}
