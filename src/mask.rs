#![allow(
    clippy::cast_ptr_alignment,
    clippy::ptr_as_ptr,
    clippy::cast_possible_wrap
)]

pub(crate) fn mask(payload: &mut [u8], mask_key: [u8; 4]) {
    #[cfg(all(target_arch = "x86_64", feature = "simd_masking"))]
    if is_x86_feature_detected!("avx2") {
        mask_avx2(payload, mask_key);
    } else {
        mask_lin(payload, mask_key);
    }

    #[cfg(not(all(target_arch = "x86_64", feature = "simd_masking")))]
    mask_lin(payload, mask_key);
}

#[cfg(all(target_arch = "x86_64", feature = "simd_masking"))]
fn mask_avx2(payload: &mut [u8], mask_key: [u8; 4]) {
    use std::arch::x86_64::{
        __m256i, _mm256_loadu_si256, _mm256_set1_epi32, _mm256_storeu_si256, _mm256_xor_si256,
    };

    let len = payload.len();
    let mask32 = u32::from_le_bytes(mask_key) as i32;
    let mask256 = unsafe { _mm256_set1_epi32(mask32) };

    let mut i = 0;
    unsafe {
        while i + 32 <= len {
            let ptr = payload.as_mut_ptr().add(i) as *mut __m256i;
            let data = _mm256_loadu_si256(ptr);
            let masked = _mm256_xor_si256(data, mask256);
            _mm256_storeu_si256(ptr, masked);
            i += 32;
        }
    }

    // tail < 32 bytes fallback
    mask_lin(&mut payload[i..], mask_key);
}

fn mask_lin(payload: &mut [u8], mask_key: [u8; 4]) {
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= mask_key[i % 4];
    }
}
