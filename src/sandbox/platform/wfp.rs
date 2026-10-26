//! Minimal Windows Filtering Platform bindings.
//!
//! `windows-sys` 0.61 does not ship `Win32_Networking_Wfp`, so the handful of
//! shapes needed for the sandbox network block are declared here. Struct
//! layouts, GUIDs, and constants were taken verbatim from the Windows SDK
//! (`shared/fwptypes.h`, `shared/fwpmtypes.h`, `um/fwpmu.h`, 10.0.26100.0).
//!
//! Semantics that matter:
//!
//! * Filters are added inside a **dynamic** session (`FWPM_SESSION_FLAG_DYNAMIC`):
//!   the moment the engine handle closes, WFP removes every filter the session
//!   added. So the engine is held open for the process lifetime — crash-safe
//!   cleanup with nothing to persist or leak.
//! * The block is keyed on `FWPM_CONDITION_ALE_PACKAGE_ID` = the sandbox's
//!   AppContainer package SID, so exactly the sandboxed process trees match.

use std::ffi::c_void;
use std::io;
use std::sync::Mutex;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    pub(crate) const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
        Self {
            data1,
            data2,
            data3,
            data4,
        }
    }
}

/// `FWPM_LAYER_ALE_AUTH_CONNECT_V4` (fwpmu.h).
const LAYER_ALE_AUTH_CONNECT_V4: Guid = Guid::new(
    0xc38d57d1,
    0x05a7,
    0x4c33,
    [0x90, 0x4f, 0x7f, 0xbc, 0xee, 0xe6, 0x0e, 0x82],
);

/// `FWPM_LAYER_ALE_AUTH_CONNECT_V6` (fwpmu.h).
const LAYER_ALE_AUTH_CONNECT_V6: Guid = Guid::new(
    0x4a72393b,
    0x319f,
    0x44bc,
    [0x84, 0xc3, 0xba, 0x54, 0xdc, 0xb3, 0xb6, 0xb4],
);

/// `FWPM_CONDITION_ALE_PACKAGE_ID` (fwpmu.h).
const CONDITION_ALE_PACKAGE_ID: Guid = Guid::new(
    0x71bc78fa,
    0xf17c,
    0x4997,
    [0xa6, 0x02, 0x6a, 0xbb, 0x26, 0x1f, 0x35, 0x1c],
);

const FWP_MATCH_EQUAL: u32 = 0;
const FWP_SID: u32 = 13;
/// `FWP_ACTION_BLOCK = FWP_ACTION_FLAG_TERMINATING | 0x1` (fwptypes.h).
const FWP_ACTION_BLOCK: u32 = 0x0000_1001;
const FWPM_SESSION_FLAG_DYNAMIC: u32 = 0x0000_0001;
const RPC_C_AUTHN_WINNT: u32 = 10;

#[repr(C)]
#[derive(Clone, Copy)]
struct FwpByteBlob {
    size: u32,
    data: *mut u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FwpValue0 {
    r#type: u32,
    value: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FwpConditionValue0 {
    r#type: u32,
    value: u64,
}

#[repr(C)]
struct FwpmDisplayData0 {
    name: *mut u16,
    description: *mut u16,
}

#[repr(C)]
struct FwpmSession0 {
    session_key: Guid,
    display_data: FwpmDisplayData0,
    flags: u32,
    txn_wait_timeout_in_msec: u32,
    process_id: u32,
    sid: *mut c_void,
    username: *mut u16,
    kernel_mode: i32,
}

#[repr(C)]
struct FwpmAction0 {
    r#type: u32,
    filter_type: Guid,
}

#[repr(C)]
struct FwpmFilterCondition0 {
    field_key: Guid,
    match_type: u32,
    condition_value: FwpConditionValue0,
}

#[repr(C)]
union ContextUnion {
    raw_context: u64,
    provider_context_key: Guid,
}

#[repr(C)]
struct FwpmFilter0 {
    filter_key: Guid,
    display_data: FwpmDisplayData0,
    flags: u32,
    provider_key: *const Guid,
    provider_data: FwpByteBlob,
    layer_key: Guid,
    sub_layer_key: Guid,
    weight: FwpValue0,
    num_filter_conditions: u32,
    filter_condition: *mut FwpmFilterCondition0,
    action: FwpmAction0,
    context: ContextUnion,
    reserved: *mut Guid,
    filter_id: u64,
    effective_weight: FwpValue0,
}

#[link(name = "fwpuclnt")]
extern "system" {
    fn FwpmEngineOpen0(
        server_name: *const u16,
        authn_service: u32,
        auth_identity: *const c_void,
        session: *const FwpmSession0,
        engine_handle: *mut *mut c_void,
    ) -> u32;
    fn FwpmEngineClose0(engine_handle: *mut c_void) -> u32;
    fn FwpmTransactionBegin0(engine_handle: *mut c_void, flags: u32) -> u32;
    fn FwpmTransactionCommit0(engine_handle: *mut c_void) -> u32;
    fn FwpmTransactionAbort0(engine_handle: *mut c_void) -> u32;
    fn FwpmFilterAdd0(
        engine_handle: *mut c_void,
        filter: *const FwpmFilter0,
        sd: *const c_void,
        id: *mut u64,
    ) -> u32;
}

struct EngineHandle(*mut c_void);
// SAFETY: the engine handle is owned by the static below and only touched
// while the mutex is held.
unsafe impl Send for EngineHandle {}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                FwpmEngineClose0(self.0);
            }
        }
    }
}

/// Held for the process lifetime: closing it would remove the dynamic
/// filters. `None` means no network block is active.
static ENGINE: Mutex<Option<EngineHandle>> = Mutex::new(None);

fn leak_wide(text: &str) -> *mut u16 {
    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<u16> = std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let ptr = wide.as_mut_ptr();
    std::mem::forget(wide);
    ptr
}

/// Blocks outbound connections (ALE_AUTH_CONNECT V4 + V6) for processes
/// carrying `package_sid`. Idempotent per process.
pub(crate) fn block_package_network(package_sid: &[u8]) -> io::Result<()> {
    let mut engine_slot = ENGINE
        .lock()
        .map_err(|_| io::Error::new(io::ErrorKind::Other, "wfp engine lock poisoned"))?;
    if engine_slot.is_some() {
        return Ok(());
    }

    unsafe {
        let mut session: FwpmSession0 = std::mem::zeroed();
        session.display_data.name = leak_wide("OSA sandbox firewall");
        session.display_data.description = leak_wide("Network block for sandboxed tool processes");
        session.flags = FWPM_SESSION_FLAG_DYNAMIC;

        let mut engine: *mut c_void = std::ptr::null_mut();
        let status = FwpmEngineOpen0(
            std::ptr::null(),
            RPC_C_AUTHN_WINNT,
            std::ptr::null(),
            &session,
            &mut engine,
        );
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }

        let begin = FwpmTransactionBegin0(engine, 0);
        if begin != 0 {
            FwpmEngineClose0(engine);
            return Err(io::Error::from_raw_os_error(begin as i32));
        }

        let mut failure = None;
        for layer in [LAYER_ALE_AUTH_CONNECT_V4, LAYER_ALE_AUTH_CONNECT_V6] {
            let mut condition = FwpmFilterCondition0 {
                field_key: CONDITION_ALE_PACKAGE_ID,
                match_type: FWP_MATCH_EQUAL,
                condition_value: FwpConditionValue0 {
                    r#type: FWP_SID,
                    value: package_sid.as_ptr() as u64,
                },
            };

            let filter = FwpmFilter0 {
                filter_key: Guid::default(),
                display_data: FwpmDisplayData0 {
                    name: leak_wide("OSA sandbox network block"),
                    description: leak_wide("Deny outbound connections for sandboxed processes"),
                },
                flags: 0,
                provider_key: std::ptr::null(),
                provider_data: FwpByteBlob {
                    size: 0,
                    data: std::ptr::null_mut(),
                },
                layer_key: layer,
                sub_layer_key: Guid::default(),
                weight: FwpValue0 {
                    r#type: 0,
                    value: 0,
                },
                num_filter_conditions: 1,
                filter_condition: &mut condition,
                action: FwpmAction0 {
                    r#type: FWP_ACTION_BLOCK,
                    filter_type: Guid::default(),
                },
                context: ContextUnion { raw_context: 0 },
                reserved: std::ptr::null_mut(),
                filter_id: 0,
                effective_weight: FwpValue0 {
                    r#type: 0,
                    value: 0,
                },
            };

            let status = FwpmFilterAdd0(engine, &filter, std::ptr::null(), std::ptr::null_mut());
            if status != 0 {
                failure = Some(status as i32);
                break;
            }
        }

        if let Some(code) = failure {
            FwpmTransactionAbort0(engine);
            FwpmEngineClose0(engine);
            return Err(io::Error::from_raw_os_error(code));
        }

        let commit = FwpmTransactionCommit0(engine);
        if commit != 0 {
            FwpmEngineClose0(engine);
            return Err(io::Error::from_raw_os_error(commit as i32));
        }

        // Keep the engine open: dynamic filters die with it.
        *engine_slot = Some(EngineHandle(engine));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_sdk() {
        // Sizes taken from the SDK headers for x64. A mismatch here means the
        // hand-rolled bindings no longer agree with the ABI.
        assert_eq!(std::mem::size_of::<Guid>(), 16);
        assert_eq!(std::mem::size_of::<FwpValue0>(), 16);
        assert_eq!(std::mem::size_of::<FwpmAction0>(), 20);
        assert_eq!(std::mem::size_of::<FwpmFilterCondition0>(), 40);
    }
}
