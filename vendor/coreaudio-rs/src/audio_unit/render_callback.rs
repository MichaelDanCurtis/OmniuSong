use objc2_audio_toolbox::{
    kAudioOutputUnitProperty_SetInputCallback, kAudioUnitProperty_SetRenderCallback,
    AURenderCallbackStruct, AudioUnitRender, AudioUnitRenderActionFlags,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp};

use super::audio_format::LinearPcmFlags;
use super::{AudioUnit, Element, Scope};
use crate::error::{self, Error};
use crate::OSStatus;
use std::convert::TryFrom;
use std::os::raw::c_void;
use std::ptr::NonNull;

pub use self::action_flags::ActionFlags;
pub use self::data::Data;

/// When `set_render_callback` is called, a closure of this type will be used to wrap the given
/// render callback function.
///
/// This allows the user to provide a custom, more rust-esque callback function type that takes
/// greater advantage of rust's type safety.
pub type InputProcFn = dyn FnMut(
    NonNull<AudioUnitRenderActionFlags>,
    NonNull<AudioTimeStamp>,
    u32,
    u32,
    *mut AudioBufferList,
) -> OSStatus;

/// This type allows us to safely wrap a boxed `RenderCallback` to use within the input proc.
pub struct InputProcFnWrapper {
    callback: Box<InputProcFn>,
}

/// Arguments given to the render callback function.
#[derive(Debug)]
pub struct Args<D> {
    /// A type wrapping the the buffer that matches the expected audio format.
    pub data: D,
    /// Timing information for the callback.
    pub time_stamp: AudioTimeStamp,
    /// TODO
    pub bus_number: u32,
    /// The number of frames in the buffer as `usize` for easier indexing.
    pub num_frames: usize,
    /// Flags for configuring audio unit rendering.
    ///
    /// This parameter lets a callback provide various hints to the audio unit.
    ///
    /// For example: if there is no audio to process, we can insert the `OUTPUT_IS_SILENCE` flag to
    /// indicate to the audio unit that the buffer does not need to be processed.
    pub flags: action_flags::Handle,
}

/// Format specific render callback data.
pub mod data {
    use objc2_core_audio_types::AudioBuffer;
    use objc2_core_audio_types::AudioBufferList;
    use std::slice;

    use super::super::Sample;
    use super::super::StreamFormat;
    use crate::audio_unit::audio_format::LinearPcmFlags;
    use std::marker::PhantomData;

    /// Audio data wrappers specific to the `AudioUnit`'s `AudioFormat`.
    pub trait Data {
        /// Check whether the stream format matches this type of data.
        fn does_stream_format_match(stream_format: &StreamFormat) -> bool;
        /// We must be able to construct Self from arguments given to the `input_proc`.
        /// # Safety
        /// TODO document how to use this function safely.
        unsafe fn from_input_proc_args(num_frames: u32, io_data: *mut AudioBufferList) -> Self;
    }

    /// A raw pointer to the audio data so that the user may handle it themselves.
    #[derive(Debug)]
    pub struct Raw {
        pub data: *mut AudioBufferList,
    }

    impl Data for Raw {
        fn does_stream_format_match(_: &StreamFormat) -> bool {
            true
        }
        unsafe fn from_input_proc_args(_num_frames: u32, io_data: *mut AudioBufferList) -> Self {
            Raw { data: io_data }
        }
    }

    /// An interleaved linear PCM buffer with samples of type `S`.
    pub struct Interleaved<S: 'static> {
        /// The audio buffer.
        pub buffer: &'static mut [S],
        pub channels: usize,
        sample_format: PhantomData<S>,
    }

    /// An interleaved linear PCM buffer with samples stored as plain bytes.
    pub struct InterleavedBytes<S: 'static> {
        /// The audio buffer.
        pub buffer: &'static mut [u8],
        pub channels: usize,
        sample_format: PhantomData<S>,
    }

    /// A wrapper around the pointer to the `mBuffers` array.
    pub struct NonInterleaved<S> {
        /// The list of audio buffers.
        buffers: &'static mut [AudioBuffer],
        /// The number of frames in each channel.
        frames: usize,
        sample_format: PhantomData<S>,
    }

    /// An iterator produced by a `NonInterleaved`, yielding a reference to each channel.
    pub struct Channels<'a, S: 'a> {
        buffers: slice::Iter<'a, AudioBuffer>,
        frames: usize,
        sample_format: PhantomData<S>,
    }

    /// An iterator produced by a `NonInterleaved`, yielding a mutable reference to each channel.
    pub struct ChannelsMut<'a, S: 'a> {
        buffers: slice::IterMut<'a, AudioBuffer>,
        frames: usize,
        sample_format: PhantomData<S>,
    }

    unsafe impl<S> Send for NonInterleaved<S> where S: Send {}

    impl<'a, S> Iterator for Channels<'a, S> {
        type Item = &'a [S];
        #[allow(non_snake_case)]
        fn next(&mut self) -> Option<Self::Item> {
            self.buffers.next().map(
                |&AudioBuffer {
                     mNumberChannels,
                     mData,
                     ..
                 }| {
                    let len = mNumberChannels as usize * self.frames;
                    let ptr = mData as *mut S;
                    unsafe { slice::from_raw_parts(ptr, len) }
                },
            )
        }
    }

    impl<'a, S> Iterator for ChannelsMut<'a, S> {
        type Item = &'a mut [S];
        #[allow(non_snake_case)]
        fn next(&mut self) -> Option<Self::Item> {
            self.buffers.next().map(
                |&mut AudioBuffer {
                     mNumberChannels,
                     mData,
                     ..
                 }| {
                    let len = mNumberChannels as usize * self.frames;
                    let ptr = mData as *mut S;
                    unsafe { slice::from_raw_parts_mut(ptr, len) }
                },
            )
        }
    }

    impl<S> NonInterleaved<S> {
        /// An iterator yielding a reference to each channel in the array.
        pub fn channels(&self) -> Channels<'_, S> {
            Channels {
                buffers: self.buffers.iter(),
                frames: self.frames,
                sample_format: PhantomData,
            }
        }

        /// An iterator yielding a mutable reference to each channel in the array.
        pub fn channels_mut(&mut self) -> ChannelsMut<'_, S> {
            ChannelsMut {
                buffers: self.buffers.iter_mut(),
                frames: self.frames,
                sample_format: PhantomData,
            }
        }
    }

    // Implementation for a non-interleaved linear PCM audio format.
    impl<S> Data for NonInterleaved<S>
    where
        S: Sample,
    {
        fn does_stream_format_match(stream_format: &StreamFormat) -> bool {
            stream_format
                .flags
                .contains(LinearPcmFlags::IS_NON_INTERLEAVED)
                && S::sample_format().does_match_flags(stream_format.flags)
        }

        #[allow(non_snake_case)]
        unsafe fn from_input_proc_args(frames: u32, io_data: *mut AudioBufferList) -> Self {
            let ptr = (*io_data).mBuffers.as_ptr() as *mut AudioBuffer;
            let len = (*io_data).mNumberBuffers as usize;
            let buffers = slice::from_raw_parts_mut(ptr, len);
            NonInterleaved {
                buffers,
                frames: frames as usize,
                sample_format: PhantomData,
            }
        }
    }

    // Implementation for an interleaved linear PCM audio format.
    impl<S> Data for Interleaved<S>
    where
        S: Sample,
    {
        fn does_stream_format_match(stream_format: &StreamFormat) -> bool {
            !stream_format
                .flags
                .contains(LinearPcmFlags::IS_NON_INTERLEAVED)
                && S::sample_format().does_match_flags(stream_format.flags)
        }

        #[allow(non_snake_case)]
        unsafe fn from_input_proc_args(frames: u32, io_data: *mut AudioBufferList) -> Self {
            // // We're expecting a single interleaved buffer which will be the first in the array.
            let AudioBuffer {
                mNumberChannels,
                mDataByteSize,
                mData,
            } = (*io_data).mBuffers[0];
            // // Ensure that the size of the data matches the size of the sample format
            // // multiplied by the number of frames.
            // //
            // // TODO: Return an Err instead of `panic`ing.
            let buffer_len = frames as usize * mNumberChannels as usize;
            let expected_size = ::std::mem::size_of::<S>() * buffer_len;
            assert!(mDataByteSize as usize == expected_size);

            let buffer: &mut [S] = {
                let buffer_ptr = mData as *mut S;
                slice::from_raw_parts_mut(buffer_ptr, buffer_len)
            };

            Interleaved {
                buffer,
                channels: mNumberChannels as usize,
                sample_format: PhantomData,
            }
        }
    }

    // Implementation for an interleaved linear PCM audio format using plain bytes.
    impl<S> Data for InterleavedBytes<S>
    where
        S: Sample,
    {
        fn does_stream_format_match(stream_format: &StreamFormat) -> bool {
            !stream_format
                .flags
                .contains(LinearPcmFlags::IS_NON_INTERLEAVED)
                && S::sample_format().does_match_flags(stream_format.flags)
        }

        #[allow(non_snake_case)]
        unsafe fn from_input_proc_args(frames: u32, io_data: *mut AudioBufferList) -> Self {
            // // We're expecting a single interleaved buffer which will be the first in the array.
            let AudioBuffer {
                mNumberChannels,
                mDataByteSize,
                mData,
            } = (*io_data).mBuffers[0];
            // // Ensure that the size of the data matches the size of the sample format
            // // multiplied by the number of frames.
            // //
            // // TODO: Return an Err instead of `panic`ing.
            let buffer_len = frames as usize * mNumberChannels as usize;
            let expected_size = ::std::mem::size_of::<S>() * buffer_len;
            assert!(mDataByteSize as usize == expected_size);

            let buffer: &mut [u8] = {
                let buffer_ptr = mData as *mut u8;
                slice::from_raw_parts_mut(buffer_ptr, mDataByteSize as usize)
            };

            InterleavedBytes {
                buffer,
                channels: mNumberChannels as usize,
                sample_format: PhantomData,
            }
        }
    }
}

pub mod action_flags {
    use objc2_audio_toolbox::AudioUnitRenderActionFlags;

    use std::fmt;

    bitflags! {
        pub struct ActionFlags: u32 {
            /// Called on a render notification Proc, which is called either before or after the
            /// render operation of the audio unit. If this flag is set, the proc is being called
            /// before the render operation is performed.
            ///
            /// **Available** in OS X v10.0 and later.
            const PRE_RENDER = AudioUnitRenderActionFlags::UnitRenderAction_PreRender.0;
            /// Called on a render notification Proc, which is called either before or after the
            /// render operation of the audio unit. If this flag is set, the proc is being called
            /// after the render operation is completed.
            ///
            /// **Available** in OS X v10.0 and later.
            const POST_RENDER = AudioUnitRenderActionFlags::UnitRenderAction_PostRender.0;
            /// This flag can be set in a render input callback (or in the audio unit's render
            /// operation itself) and is used to indicate that the render buffer contains only
            /// silence. It can then be used by the caller as a hint to whether the buffer needs to
            /// be processed or not.
            ///
            /// **Available** in OS X v10.2 and later.
            const OUTPUT_IS_SILENCE = AudioUnitRenderActionFlags::UnitRenderAction_OutputIsSilence.0;
            /// This is used with offline audio units (of type 'auol'). It is used when an offline
            /// unit is being preflighted, which is performed prior to when the actual offline
            /// rendering actions are performed. It is used for those cases where the offline
            /// process needs it (for example, with an offline unit that normalizes an audio file,
            /// it needs to see all of the audio data first before it can perform its
            /// normalization).
            ///
            /// **Available** in OS X v10.3 and later.
            const OFFLINE_PREFLIGHT = AudioUnitRenderActionFlags::OfflineUnitRenderAction_Preflight.0;
            /// Once an offline unit has been successfully preflighted, it is then put into its
            /// render mode. This flag is set to indicate to the audio unit that it is now in that
            /// state and that it should perform processing on the input data.
            ///
            /// **Available** in OS X v10.3 and later.
            const OFFLINE_RENDER = AudioUnitRenderActionFlags::OfflineUnitRenderAction_Render.0;
            /// This flag is set when an offline unit has completed either its preflight or
            /// performed render operation.
            ///
            /// **Available** in OS X v10.3 and later.
            const OFFLINE_COMPLETE = AudioUnitRenderActionFlags::OfflineUnitRenderAction_Complete.0;
            /// If this flag is set on the post-render call an error was returned by the audio
            /// unit's render operation. In this case, the error can be retrieved through the
            /// `lastRenderError` property and the audio data in `ioData` handed to the post-render
            /// notification will be invalid.
            ///
            /// **Available** in OS X v10.5 and later.
            const POST_RENDER_ERROR = AudioUnitRenderActionFlags::UnitRenderAction_PostRenderError.0;
            /// If this flag is set, then checks that are done on the arguments provided to render
            /// are not performed. This can be useful to use to save computation time in situations
            /// where you are sure you are providing the correct arguments and structures to the
            /// various render calls.
            ///
            /// **Available** in OS X v10.7 and later.
            const DO_NOT_CHECK_RENDER_ARGS = AudioUnitRenderActionFlags::UnitRenderAction_DoNotCheckRenderArgs.0;
        }
    }

    /// A safe handle around the `AudioUnitRenderActionFlags` pointer provided by the render
    /// callback.
    ///
    /// This type lets a callback provide various hints to the audio unit.
    ///
    /// For example: if there is no audio to process, we can insert the `OUTPUT_IS_SILENCE` flag to
    /// indicate to the audio unit that the buffer does not need to be processed.
    pub struct Handle {
        ptr: *mut AudioUnitRenderActionFlags,
    }

    impl fmt::Debug for Handle {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            if self.ptr.is_null() {
                write!(f, "{:?}", self.ptr)
            } else {
                unsafe { write!(f, "{:?}", *self.ptr) }
            }
        }
    }

    impl Handle {
        /// Retrieve the current state of the `ActionFlags`.
        pub fn get(&self) -> ActionFlags {
            ActionFlags::from_bits_truncate(unsafe { *self.ptr }.0)
        }

        fn set(&mut self, flags: ActionFlags) {
            unsafe { (*self.ptr).0 = flags.bits() }
        }

        /// The raw value of the flags currently stored.
        pub fn bits(&self) -> u32 {
            self.get().bits()
        }

        /// Returns `true` if no flags are currently stored.
        pub fn is_empty(&self) -> bool {
            self.get().is_empty()
        }

        /// Returns `true` if all flags are currently stored.
        pub fn is_all(&self) -> bool {
            self.get().is_all()
        }

        /// Returns `true` if there are flags common to both `self` and `other`.
        pub fn intersects(&self, other: ActionFlags) -> bool {
            self.get().intersects(other)
        }

        /// Returns `true` if all of the flags in `other` are contained within `self`.
        pub fn contains(&self, other: ActionFlags) -> bool {
            self.get().contains(other)
        }

        /// Insert the specified flags in-place.
        pub fn insert(&mut self, other: ActionFlags) {
            let mut flags = self.get();
            flags.insert(other);
            self.set(flags);
        }

        /// Remove the specified flags in-place.
        pub fn remove(&mut self, other: ActionFlags) {
            let mut flags = self.get();
            flags.remove(other);
            self.set(flags);
        }

        /// Toggles the specified flags in-place.
        pub fn toggle(&mut self, other: ActionFlags) {
            let mut flags = self.get();
            flags.toggle(other);
            self.set(flags);
        }

        /// Wrap the given pointer with a `Handle`.
        pub fn from_ptr(ptr: *mut AudioUnitRenderActionFlags) -> Self {
            Handle { ptr }
        }
    }

    unsafe impl Send for Handle {}

    impl ::std::fmt::Display for ActionFlags {
        fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
            write!(
                f,
                "{:?}",
                match AudioUnitRenderActionFlags(self.bits()) {
                    AudioUnitRenderActionFlags::UnitRenderAction_PreRender => "PRE_RENDER",
                    AudioUnitRenderActionFlags::UnitRenderAction_PostRender => "POST_RENDER",
                    AudioUnitRenderActionFlags::UnitRenderAction_OutputIsSilence =>
                        "OUTPUT_IS_SILENCE",
                    AudioUnitRenderActionFlags::OfflineUnitRenderAction_Preflight =>
                        "OFFLINE_PREFLIGHT",
                    AudioUnitRenderActionFlags::OfflineUnitRenderAction_Render => "OFFLINE_RENDER",
                    AudioUnitRenderActionFlags::OfflineUnitRenderAction_Complete =>
                        "OFFLINE_COMPLETE",
                    AudioUnitRenderActionFlags::UnitRenderAction_PostRenderError =>
                        "POST_RENDER_ERROR",
                    AudioUnitRenderActionFlags::UnitRenderAction_DoNotCheckRenderArgs =>
                        "DO_NOT_CHECK_RENDER_ARGS",
                    _ => "<Unknown ActionFlags>",
                }
            )
        }
    }
}

impl AudioUnit {
    /// Pass a render callback (aka "Input Procedure") to the **AudioUnit**.
    pub fn set_render_callback<F, D>(&mut self, mut f: F) -> Result<(), Error>
    where
        F: FnMut(Args<D>) -> Result<(), ()> + 'static,
        D: Data,
    {
        // First, we'll retrieve the stream format so that we can ensure that the given callback
        // format matches the audio unit's format.
        let stream_format = self.output_stream_format()?;

        // If the stream format does not match, return an error indicating this.
        if !D::does_stream_format_match(&stream_format) {
            return Err(Error::RenderCallbackBufferFormatDoesNotMatchAudioUnitStreamFormat);
        }

        // Here, we call the given render callback function within a closure that matches the
        // arguments of the required coreaudio "input_proc".
        //
        // This allows us to take advantage of rust's type system and provide format-specific
        // `Args` types which can be checked at compile time.
        let input_proc_fn = move |io_action_flags: NonNull<AudioUnitRenderActionFlags>,
                                  in_time_stamp: NonNull<AudioTimeStamp>,
                                  in_bus_number: u32,
                                  in_number_frames: u32,
                                  io_data: *mut AudioBufferList|
              -> OSStatus {
            let args = unsafe {
                let data = D::from_input_proc_args(in_number_frames, io_data);
                let flags = action_flags::Handle::from_ptr(io_action_flags.as_ptr());
                Args {
                    data,
                    time_stamp: in_time_stamp.read(),
                    flags,
                    bus_number: in_bus_number,
                    num_frames: in_number_frames as usize,
                }
            };

            match f(args) {
                Ok(()) => 0,
                Err(()) => error::Error::Unspecified.as_os_status(),
            }
        };

        let input_proc_fn_wrapper = Box::new(InputProcFnWrapper {
            callback: Box::new(input_proc_fn),
        });

        // Setup render callback. Notice that we relinquish ownership of the Callback
        // here so that it can be used as the C render callback via a void pointer.
        // We do however store the *mut so that we can convert back to a Box<InputProcFnWrapper>
        // within our AudioUnit's Drop implementation (otherwise it would leak).
        let input_proc_fn_wrapper_ptr = Box::into_raw(input_proc_fn_wrapper) as *mut c_void;

        let render_callback = AURenderCallbackStruct {
            inputProc: Some(input_proc),
            inputProcRefCon: input_proc_fn_wrapper_ptr,
        };

        self.set_property(
            kAudioUnitProperty_SetRenderCallback,
            Scope::Input,
            Element::Output,
            Some(&render_callback),
        )?;

        self.free_render_callback();
        self.maybe_render_callback = Some(input_proc_fn_wrapper_ptr as *mut InputProcFnWrapper);
        Ok(())
    }

    /// Pass an input callback (aka "Input Procedure") to the **AudioUnit**.
    pub fn set_input_callback<F, D>(&mut self, mut f: F) -> Result<(), Error>
    where
        F: FnMut(Args<D>) -> Result<(), ()> + 'static,
        D: Data,
    {
        // First, we'll retrieve the stream format so that we can ensure that the given callback
        // format matches the audio unit's format.
        let stream_format = self.input_stream_format()?;

        // If the stream format does not match, return an error indicating this.
        if !D::does_stream_format_match(&stream_format) {
            return Err(Error::RenderCallbackBufferFormatDoesNotMatchAudioUnitStreamFormat);
        }

        // Interleaved or non-interleaved?
        let non_interleaved = stream_format
            .flags
            .contains(LinearPcmFlags::IS_NON_INTERLEAVED);

        // Pre-allocate a buffer list for input stream.
        //
        // First, get the current buffer size for pre-allocating the `AudioBuffer`s.
        #[cfg(target_os = "macos")]
        let buffer_frame_size: u32 = {
            let id = objc2_core_audio::kAudioDevicePropertyBufferFrameSize;
            let buffer_frame_size: u32 = self.get_property(id, Scope::Global, Element::Output)?;
            buffer_frame_size
        };
        #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "visionos"))]
        let buffer_frame_size: u32 = {
            let id = objc2_audio_toolbox::kAudioSessionProperty_CurrentHardwareIOBufferDuration;
            let seconds: f32 = super::audio_session_get_property(id)?;
            let id = objc2_audio_toolbox::kAudioSessionProperty_CurrentHardwareSampleRate;
            let sample_rate: f64 = super::audio_session_get_property(id)?;
            (sample_rate * seconds as f64).round() as u32
        };
        let sample_bytes = stream_format.sample_format.size_in_bytes();
        let n_channels = stream_format.channels;
        if non_interleaved && n_channels > 1 {
            return Err(Error::NonInterleavedInputOnlySupportsMono);
        }

        // OmniuSong: fixed storage is allocated before activation. Never query
        // properties, reconstruct a Vec, resize or allocate inside input_proc.
        const INPUT_CAPACITY_FRAMES: u32 = 16_384;
        if buffer_frame_size > INPUT_CAPACITY_FRAMES {
            return Err(Error::Unspecified);
        }
        let capacity = bounded_input_bytes(
            INPUT_CAPACITY_FRAMES,
            sample_bytes,
            n_channels,
            16 * 1024 * 1024,
        )
        .ok_or(Error::Unspecified)? as usize;
        let mut data = input_storage(capacity)?;
        let audio_buffer = AudioBuffer {
            mDataByteSize: 0,
            mNumberChannels: n_channels,
            mData: data.as_mut_ptr() as *mut _,
        };

        let audio_buffer_list = Box::new(AudioBufferList {
            mNumberBuffers: 1,
            mBuffers: [audio_buffer],
        });

        // Relinquish ownership of the audio buffer list. Instead, we'll store a raw pointer and
        // convert it back into a `Box` when `free_input_callback` is next called.
        let audio_buffer_list_ptr = Box::into_raw(audio_buffer_list);

        // Here, we call the given input callback function within a closure that matches the
        // arguments of the required coreaudio "input_proc".
        //
        // This allows us to take advantage of rust's type system and provide format-specific
        // `Args` types which can be checked at compile time.
        let audio_unit = self.instance;
        let input_proc_fn = move |io_action_flags: NonNull<AudioUnitRenderActionFlags>,
                                  in_time_stamp: NonNull<AudioTimeStamp>,
                                  in_bus_number: u32,
                                  in_number_frames: u32,
                                  _io_data: *mut AudioBufferList|
              -> OSStatus {
            let requested =
                bounded_input_bytes(in_number_frames, sample_bytes, n_channels, capacity);
            let render_status = match requested {
                Some(bytes) => unsafe {
                    (*audio_buffer_list_ptr).mBuffers[0].mDataByteSize = bytes;
                    AudioUnitRender(
                        audio_unit,
                        io_action_flags.as_ptr(),
                        in_time_stamp,
                        in_bus_number,
                        in_number_frames,
                        NonNull::new(audio_buffer_list_ptr).unwrap(),
                    )
                },
                None => error::Error::Unspecified.as_os_status(),
            };
            // Deliver an empty-data sentinel on failure so the application can
            // latch unknown loss instead of sealing a silently incomplete take.
            // CPAL forwards mDataByteSize; it does not consume Args::num_frames.
            if render_status != 0 {
                unsafe {
                    (*audio_buffer_list_ptr).mBuffers[0].mDataByteSize = 0;
                }
            }
            let callback_frames = if render_status == 0 {
                in_number_frames
            } else {
                0
            };

            let args = unsafe {
                let data = D::from_input_proc_args(callback_frames, audio_buffer_list_ptr);
                let flags = action_flags::Handle::from_ptr(io_action_flags.as_ptr());
                Args {
                    data,
                    time_stamp: in_time_stamp.read(),
                    flags,
                    bus_number: in_bus_number,
                    num_frames: callback_frames as usize,
                }
            };

            let result = f(args);
            if render_status != 0 {
                return render_status;
            }
            match result {
                Ok(()) => 0,
                Err(()) => error::Error::Unspecified.as_os_status(),
            }
        };

        let input_proc_fn_wrapper = Box::new(InputProcFnWrapper {
            callback: Box::new(input_proc_fn),
        });

        // Setup input callback. Notice that we relinquish ownership of the Callback
        // here so that it can be used as the C render callback via a void pointer.
        // We do however store the *mut so that we can convert back to a Box<InputProcFnWrapper>
        // within our AudioUnit's Drop implementation (otherwise it would leak).
        let input_proc_fn_wrapper_ptr = Box::into_raw(input_proc_fn_wrapper) as *mut c_void;

        let render_callback = AURenderCallbackStruct {
            inputProc: Some(input_proc),
            inputProcRefCon: input_proc_fn_wrapper_ptr,
        };

        self.set_property(
            kAudioOutputUnitProperty_SetInputCallback,
            Scope::Global,
            Element::Output,
            Some(&render_callback),
        )?;

        let input_callback = super::InputCallback {
            _input_data: data,
            buffer_list: audio_buffer_list_ptr,
            callback: input_proc_fn_wrapper_ptr as *mut InputProcFnWrapper,
        };
        self.free_input_callback();
        self.maybe_input_callback = Some(input_callback);
        Ok(())
    }

    /// Retrieves ownership over the render callback and returns it where it can be re-used or
    /// safely dropped.
    pub fn free_render_callback(&mut self) -> Option<Box<InputProcFnWrapper>> {
        if let Some(callback) = self.maybe_render_callback.take() {
            // Here, we transfer ownership of the callback back to the current scope so that it
            // is dropped and cleaned up. Without this line, we would leak the Boxed callback.
            let callback: Box<InputProcFnWrapper> = unsafe { Box::from_raw(callback) };
            return Some(callback);
        }
        None
    }

    /// Retrieves ownership over the input callback and returns it where it can be re-used or
    /// safely dropped.
    pub fn free_input_callback(&mut self) -> Option<Box<InputProcFnWrapper>> {
        if let Some(input_callback) = self.maybe_input_callback.take() {
            let super::InputCallback {
                buffer_list,
                callback,
                _input_data,
            } = input_callback;
            unsafe {
                // Take ownership over the AudioBufferList in order to safely free it.
                let buffer_list: Box<AudioBufferList> = Box::from_raw(buffer_list);
                // _input_data owns the original Vec, with its true capacity.
                // Drop it here on the AudioUnit owner/control thread.
                drop(_input_data);
                drop(buffer_list);
                // Take ownership over the callback so that it can be freed.
                let callback: Box<InputProcFnWrapper> = Box::from_raw(callback);
                return Some(callback);
            }
        }
        None
    }
}

/// Callback procedure that will be called each time our audio_unit requests audio.
extern "C-unwind" fn input_proc(
    in_ref_con: NonNull<c_void>,
    io_action_flags: NonNull<AudioUnitRenderActionFlags>,
    in_time_stamp: NonNull<AudioTimeStamp>,
    in_bus_number: u32,
    in_number_frames: u32,
    io_data: *mut AudioBufferList,
) -> OSStatus {
    let wrapper = unsafe { in_ref_con.cast::<InputProcFnWrapper>().as_mut() };
    (wrapper.callback)(
        io_action_flags,
        in_time_stamp,
        in_bus_number,
        in_number_frames,
        io_data,
    )
}

// OmniuSong: the callback uses this checked arithmetic only, with no allocation.
#[cfg(test)]
#[path = "../omniusong_review_tests.rs"]
mod omniusong_independent_review_tests;

fn bounded_input_bytes(
    frames: u32,
    sample_bytes: usize,
    channels: u32,
    capacity: usize,
) -> Option<u32> {
    if frames == 0 || sample_bytes == 0 || channels == 0 {
        return None;
    }
    let bytes = (frames as usize)
        .checked_mul(sample_bytes)?
        .checked_mul(channels as usize)?;
    if bytes > capacity {
        return None;
    }
    u32::try_from(bytes).ok()
}

// The typed callback views may contain f64/f32/integer samples. Owning u64
// elements guarantees their alignment through Rust's allocation contract,
// without relying on a particular malloc implementation over-aligning u8.
fn input_storage(capacity_bytes: usize) -> Result<Vec<u64>, Error> {
    if capacity_bytes == 0 || capacity_bytes > 16 * 1024 * 1024 || capacity_bytes % 8 != 0 {
        return Err(Error::Unspecified);
    }
    Ok(vec![0u64; capacity_bytes / 8])
}

#[cfg(test)]
mod omniusong_input_buffer_tests {
    use super::{bounded_input_bytes, input_storage};
    #[test]
    fn variable_frames_use_one_fixed_allocation_and_reject_oversize() {
        let capacity = 16_384 * 8 * 4;
        for frames in [1, 64, 256, 128, 4096, 16_384, 32] {
            assert_eq!(
                bounded_input_bytes(frames, 4, 8, capacity),
                Some(frames * 32)
            );
        }
        assert_eq!(bounded_input_bytes(16_385, 4, 8, capacity), None);
        assert_eq!(bounded_input_bytes(0, 4, 8, capacity), None);
        assert_eq!(
            bounded_input_bytes(u32::MAX, usize::MAX, u32::MAX, usize::MAX),
            None
        );
    }
    #[test]
    fn original_vec_layout_survives_smaller_view_lengths() {
        let owned = input_storage(8192).unwrap();
        let initial_capacity = owned.capacity();
        for frames in [1024, 16, 512, 1] {
            assert!(bounded_input_bytes(frames, 4, 2, owned.len() * 8).is_some());
            assert_eq!(owned.len() * 8, 8192);
            assert_eq!(owned.capacity(), initial_capacity);
        }
        drop(owned);
    }
}
