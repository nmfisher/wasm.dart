use alloc::alloc::{Layout, alloc as allocate, dealloc, handle_alloc_error, realloc};

#[unsafe(no_mangle)]
pub extern "C" fn dart_realloc(
    old_ptr: *mut u8,
    old_len: usize,
    align: usize,
    new_len: usize,
) -> *mut u8 {
    let layout;
    let ptr = unsafe {
        if old_len == 0 {
            if new_len == 0 {
                return align as *mut u8;
            }
            layout = Layout::from_size_align_unchecked(new_len, align);
            allocate(layout)
        } else {
            debug_assert_ne!(new_len, 0, "non-zero old_len requires non-zero new_len!");
            layout = Layout::from_size_align_unchecked(old_len, align);
            realloc(old_ptr, layout, new_len)
        }
    };
    if ptr.is_null() {
        // Print a nice message in debug mode, but in release mode don't
        // pull in so many dependencies related to printing so just emit an
        // `unreachable` instruction.
        if cfg!(debug_assertions) {
            handle_alloc_error(layout);
        } else {
            #[cfg(target_arch = "wasm32")]
            core::arch::wasm32::unreachable();
            #[cfg(not(target_arch = "wasm32"))]
            unreachable!();
        }
    }
    return ptr;
}

#[unsafe(no_mangle)]
pub extern "C" fn dart_free(ptr: *mut u8, num_bytes: usize, align: usize) {
    // Talc requires a nonzero layout: it documents on `grow`/`shrink` that the
    // caller must ensure the size is greater than zero, and a zero-size
    // `dealloc` sends it a dangling pointer (see `dart_realloc`, which returns
    // `align` for exactly this case). Zero-size blocks were never really
    // allocated, so there is nothing to hand back.
    if num_bytes == 0 {
        return;
    }
    unsafe { dealloc(ptr, Layout::from_size_align_unchecked(num_bytes, align)) }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::dart_free;
    use core::alloc::{GlobalAlloc, Layout};
    use core::sync::atomic::{AtomicUsize, Ordering};

    static DEALLOCS: AtomicUsize = AtomicUsize::new(0);
    static ZERO_SIZE_DEALLOCS: AtomicUsize = AtomicUsize::new(0);

    /// Forwards to the system allocator but records what `dart_free` asked for,
    /// so a test can assert that zero-size blocks never reach the allocator.
    struct RecordingAlloc;

    unsafe impl GlobalAlloc for RecordingAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            unsafe { std::alloc::System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            DEALLOCS.fetch_add(1, Ordering::SeqCst);
            if layout.size() == 0 {
                ZERO_SIZE_DEALLOCS.fetch_add(1, Ordering::SeqCst);
            }
            unsafe { std::alloc::System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static ALLOC: RecordingAlloc = RecordingAlloc;

    /// The pointer `dart_realloc` hands back for a zero-size allocation is the
    /// alignment value, which is what an empty `AllocatedString` frees.
    fn dangling_zero_size_ptr(align: usize) -> *mut u8 {
        align as *mut u8
    }

    #[test]
    fn dart_free_ignores_zero_size_blocks() {
        let ptr = dangling_zero_size_ptr(2);

        DEALLOCS.store(0, Ordering::SeqCst);
        ZERO_SIZE_DEALLOCS.store(0, Ordering::SeqCst);

        dart_free(ptr, 0, 2);

        assert_eq!(
            DEALLOCS.load(Ordering::SeqCst),
            0,
            "a zero-size free must not reach the allocator"
        );
        assert_eq!(
            ZERO_SIZE_DEALLOCS.load(Ordering::SeqCst),
            0,
            "talc requires a nonzero layout; a zero-size dealloc must never be attempted"
        );
    }

    #[test]
    fn dart_free_still_frees_nonzero_blocks() {
        // Go through dart_realloc to get a pointer that is genuinely allocated,
        // so the dealloc below is well defined.
        let ptr = super::dart_realloc(core::ptr::null_mut(), 0, 2, 8);
        assert!(!ptr.is_null());

        DEALLOCS.store(0, Ordering::SeqCst);
        ZERO_SIZE_DEALLOCS.store(0, Ordering::SeqCst);

        dart_free(ptr, 8, 2);

        assert_eq!(
            DEALLOCS.load(Ordering::SeqCst),
            1,
            "a normal free must still reach the allocator"
        );
        assert_eq!(ZERO_SIZE_DEALLOCS.load(Ordering::SeqCst), 0);
    }
}
