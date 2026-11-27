//! Electron reads some service credentials from environment variables, not
//! from a store of its own (src/utils/envLookup.ts). Explicit native imports
//! resolve one allowlisted name the same way, in process, with no shell.

/// Electron's order for one allowlisted name: on Windows a nonempty trimmed
/// Machine-scope value wins, then a nonempty process value is used verbatim.
/// Other platforms read only the process environment, as Electron does.
pub(crate) fn electron_variable(name: &'static str) -> Option<String> {
    #[cfg(windows)]
    let machine = machine(name);
    #[cfg(not(windows))]
    let machine = None;
    resolve(machine, || std::env::var(name).ok())
}

// Match src/utils/envLookup.ts: trim Machine, then use nonempty process verbatim.
fn resolve(machine: Option<String>, process: impl FnOnce() -> Option<String>) -> Option<String> {
    machine
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| process().filter(|value| !value.is_empty()))
}

#[cfg(windows)]
fn machine(name: &str) -> Option<String> {
    use windows_sys::Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW},
    };
    let key: Vec<_> = "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let name: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
    // Windows environment variables have a 32767-character limit. Fixed bounded
    // storage avoids trusting a registry-provided allocation length.
    let mut buffer = vec![0u16; 32768];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: terminated key/name, valid predefined read-only handle and a
    // writable aligned buffer with its full byte capacity. No secret subprocess.
    let result = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            // Electron uses Windows PowerShell's .NET Framework RegistryKey
            // GetValue, which expands REG_EXPAND_SZ using this process env.
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if result != ERROR_SUCCESS
        || bytes < 2
        || bytes as usize > buffer.len() * 2
        || !bytes.is_multiple_of(2)
    {
        return None;
    }
    let units = bytes as usize / 2;
    if buffer[units - 1] != 0 {
        return None;
    }
    String::from_utf16(&buffer[..units - 1]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_resolution_matches_electron_machine_precedence_and_process_fallback() {
        assert_eq!(
            resolve(Some(" machine \r\n".into()), || panic!("machine wins")),
            Some("machine".into())
        );
        for machine in [None, Some(" \r\n".into())] {
            assert_eq!(
                resolve(machine, || Some("process".into())),
                Some("process".into())
            );
        }
        // Process values are not trimmed by Electron. Invalid token whitespace is
        // rejected by the complete credential validator, not silently repaired.
        assert_eq!(
            resolve(None, || Some(" process ".into())),
            Some(" process ".into())
        );
        assert!(resolve(None, || Some(String::new())).is_none());
        assert!(resolve(None, || None).is_none());
    }

    #[test]
    #[cfg(not(windows))]
    fn other_platforms_read_only_the_process_variable() {
        // A name no environment defines: nothing is invented or read elsewhere.
        assert!(electron_variable("BUDDY_SYNTHETIC_UNSET_ELECTRON_VARIABLE").is_none());
    }
}
