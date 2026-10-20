//! Independent pure buffer/view regressions. No AudioUnit or audio device is opened.
use super::{bounded_input_bytes, data, input_storage};
use data::Data;
use objc2_core_audio_types::{AudioBuffer, AudioBufferList};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

struct CountingAllocator;
thread_local! {
    static COUNTS: Cell<(bool, usize, usize)> = const { Cell::new((false, 0, 0)) };
}
fn record(a: usize, d: usize) {
    let _ = COUNTS.try_with(|cell| {
        let (active, allocations, deallocations) = cell.get();
        if active {
            cell.set((true, allocations + a, deallocations + d));
        }
    });
}
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(1, 0);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(1, 0);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record(0, 1);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(1, 1);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn omniusong_review_checked_extents_reject_zero_overflow_and_exact_capacity_plus_one() {
    for channels in [1, 2, 8, 16] {
        for width in [1, 2, 3, 4, 8] {
            let capacity = 16_384 * channels as usize * width;
            assert_eq!(
                bounded_input_bytes(16_384, width, channels, capacity),
                Some(capacity as u32)
            );
            assert_eq!(bounded_input_bytes(16_385, width, channels, capacity), None);
            assert_eq!(bounded_input_bytes(0, width, channels, capacity), None);
            assert_eq!(bounded_input_bytes(1, 0, channels, capacity), None);
            assert_eq!(bounded_input_bytes(1, width, 0, capacity), None);
        }
    }
    assert_eq!(bounded_input_bytes(u32::MAX, 8, u32::MAX, usize::MAX), None);
    assert_eq!(bounded_input_bytes(1, usize::MAX, 2, usize::MAX), None);
    assert_eq!(bounded_input_bytes(u32::MAX, 2, 1, usize::MAX), None);
}

#[test]
fn omniusong_review_variable_native_views_keep_one_aligned_owner_without_hot_path_allocation() {
    // Exercise the actual patched allocation constructor and typed public view;
    // this fixture does not run AudioUnitRender or a device callback.
    let byte_capacity = 16_384 * 2 * std::mem::size_of::<f32>();
    let mut owned = input_storage(byte_capacity).unwrap();
    let original_ptr = owned.as_mut_ptr();
    let original_len = owned.len();
    let original_capacity = owned.capacity();
    assert_eq!(original_ptr as usize % std::mem::align_of::<f64>(), 0);
    assert_eq!(original_ptr as usize % std::mem::align_of::<f32>(), 0);
    assert_eq!(owned.len() * std::mem::size_of::<u64>(), byte_capacity);
    let mut buffers = AudioBufferList {
        mNumberBuffers: 1,
        mBuffers: [AudioBuffer {
            mNumberChannels: 2,
            mDataByteSize: 0,
            mData: original_ptr.cast(),
        }],
    };
    let mut observed = [0_usize; 7];
    COUNTS.with(|cell| cell.set((true, 0, 0)));
    for (index, frames) in [16_384, 1, 256, 1024, 2, 16_384, 0].iter().enumerate() {
        let size = bounded_input_bytes(*frames, 4, 2, byte_capacity);
        buffers.mBuffers[0].mDataByteSize = size.unwrap_or(0);
        let delivered_frames = if size.is_some() { *frames } else { 0 };
        // SAFETY: the single buffer points to the still-live aligned allocation;
        // its checked byte view agrees with delivered_frames, including zero.
        let view = unsafe {
            data::Interleaved::<f32>::from_input_proc_args(delivered_frames, &mut buffers)
        };
        observed[index] = view.buffer.len();
    }
    let counts = COUNTS.with(|cell| cell.replace((false, 0, 0)));
    assert_eq!((counts.1, counts.2), (0, 0));
    assert_eq!(observed, [32_768, 2, 512, 2048, 4, 32_768, 0]);
    assert_eq!(owned.as_mut_ptr(), original_ptr);
    assert_eq!(owned.len(), original_len);
    assert_eq!(owned.capacity(), original_capacity);
    assert!(owned.iter().all(|value| *value == 0));
    drop(owned); // The unique owner retains its original layout after zero view.
    assert!(input_storage(0).is_err());
    assert!(input_storage(7).is_err());
    assert!(input_storage(16 * 1024 * 1024 + 8).is_err());
}

#[test]
fn omniusong_review_failure_sentinel_is_empty_for_both_raw_and_typed_clients() {
    let mut owned = [i16::MIN, i16::MAX, 123, -123];
    let mut buffers = AudioBufferList {
        mNumberBuffers: 1,
        mBuffers: [AudioBuffer {
            mNumberChannels: 2,
            mDataByteSize: 0,
            mData: owned.as_mut_ptr().cast(),
        }],
    };
    // The integration failure branch must supply BOTH zero bytes and zero frames.
    // SAFETY: valid aligned backing and a zero-length view of a live allocation.
    let typed = unsafe { data::Interleaved::<i16>::from_input_proc_args(0, &mut buffers) };
    assert!(typed.buffer.is_empty());
    assert_eq!(typed.channels, 2);
    // SAFETY: Raw retains only the valid buffer-list pointer during this scope.
    let raw = unsafe { data::Raw::from_input_proc_args(0, &mut buffers) };
    assert_eq!(unsafe { (*raw.data).mBuffers[0].mDataByteSize }, 0);
    assert_eq!(unsafe { (*raw.data).mBuffers[0].mNumberChannels }, 2);
    assert_eq!(owned, [i16::MIN, i16::MAX, 123, -123]);
}
