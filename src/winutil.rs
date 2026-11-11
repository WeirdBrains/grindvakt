#[cfg(target_os = "windows")]
use anyhow::Result;
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{HLOCAL, LocalFree};
#[cfg(target_os = "windows")]
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_LOCAL_MACHINE, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
    CryptUnprotectData,
};

#[cfg(target_os = "windows")]
fn to_blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    }
}

#[cfg(target_os = "windows")]
pub fn dpapi_protect_machine_scope(data: &[u8]) -> Result<Vec<u8>> {
    unsafe {
        let in_blob = to_blob(data);
        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let flags = CRYPTPROTECT_LOCAL_MACHINE | CRYPTPROTECT_UI_FORBIDDEN;
        CryptProtectData(&in_blob, None, None, None, None, flags, &mut out_blob)
            .map_err(|e| anyhow::anyhow!("CryptProtectData failed: {e}"))?;
        let out = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        // Free with LocalFree
        let _ = LocalFree(HLOCAL(out_blob.pbData as *mut core::ffi::c_void));
        Ok(out)
    }
}

#[cfg(target_os = "windows")]
pub fn dpapi_unprotect(data: &[u8]) -> Result<Vec<u8>> {
    unsafe {
        let in_blob = to_blob(data);
        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let flags = CRYPTPROTECT_UI_FORBIDDEN;
        CryptUnprotectData(&in_blob, None, None, None, None, flags, &mut out_blob)
            .map_err(|e| anyhow::anyhow!("CryptUnprotectData failed: {e}"))?;
        let out = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(out_blob.pbData as *mut core::ffi::c_void));
        Ok(out)
    }
}

#[cfg(test)]
#[cfg(target_os = "windows")]
mod tests {
    use super::*;
    #[test]
    fn dpapi_roundtrip_machine_scope() {
        let data = b"hello dpapi";
        let enc = dpapi_protect_machine_scope(data).unwrap();
        assert_ne!(enc, data);
        let dec = dpapi_unprotect(&enc).unwrap();
        assert_eq!(dec, data);
    }
}
