//! Read-only CoreAudio device facts: whether a device is running for any
//! process (what the microphone indicator follows), the output latency, and
//! the output volume.

use std::ffi::c_void;

type ObjectId = u32;

#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyDataSize(
        object: ObjectId,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioObjectGetPropertyData(
        object: ObjectId,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
}

const fn code(name: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*name)
}

const SYSTEM: ObjectId = 1;
const GLOBAL: u32 = code(b"glob");
const OUTPUT: u32 = code(b"outp");

fn read<T: Copy + Default>(object: ObjectId, selector: u32, scope: u32, element: u32) -> Option<T> {
    let address = Address {
        selector,
        scope,
        element,
    };
    let mut value = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    // SAFETY: `value` is a plain value of exactly `size` bytes.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &address,
            0,
            std::ptr::null(),
            &mut size,
            (&raw mut value).cast(),
        )
    };
    (status == 0 && size as usize == std::mem::size_of::<T>()).then_some(value)
}

fn default_device(input: bool) -> Option<ObjectId> {
    let selector = if input { code(b"dIn ") } else { code(b"dOut") };
    read::<ObjectId>(SYSTEM, selector, GLOBAL, 0).filter(|id| *id != 0)
}

/// Whether the default input (or output) device is running for any process.
pub(crate) fn running_somewhere(input: bool) -> Option<bool> {
    let device = default_device(input)?;
    read::<u32>(device, code(b"gone"), GLOBAL, 0).map(|running| running != 0)
}

/// What the default output device adds between a rendered sample and the
/// speaker, in its own frames: device latency, safety offset, buffer, and the
/// first output stream's latency.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct OutputLatency {
    pub rate: f64,
    pub device: u32,
    pub safety: u32,
    pub buffer: u32,
    pub stream: u32,
}

impl OutputLatency {
    pub(crate) fn frames(&self) -> u64 {
        u64::from(self.device)
            + u64::from(self.safety)
            + u64::from(self.buffer)
            + u64::from(self.stream)
    }

    /// In frames at `rate`.
    pub(crate) fn at(&self, rate: f64) -> u64 {
        if self.rate > 0.0 && rate > 0.0 {
            (self.frames() as f64 * rate / self.rate).round() as u64
        } else {
            self.frames()
        }
    }
}

pub(crate) fn output_latency() -> Option<OutputLatency> {
    let device = default_device(false)?;
    let streams = code(b"stm#");
    let address = Address {
        selector: streams,
        scope: OUTPUT,
        element: 0,
    };
    let mut size = 0_u32;
    // SAFETY: a size query.
    let status =
        unsafe { AudioObjectGetPropertyDataSize(device, &address, 0, std::ptr::null(), &mut size) };
    let mut stream = 0;
    if status == 0 && size as usize >= std::mem::size_of::<ObjectId>() {
        let mut ids = vec![0 as ObjectId; size as usize / std::mem::size_of::<ObjectId>()];
        // SAFETY: `ids` holds `size` bytes.
        let status = unsafe {
            AudioObjectGetPropertyData(
                device,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                ids.as_mut_ptr().cast(),
            )
        };
        if status == 0 {
            stream = ids
                .first()
                .and_then(|id| read::<u32>(*id, code(b"ltnc"), GLOBAL, 0))
                .unwrap_or(0);
        }
    }
    Some(OutputLatency {
        rate: read::<f64>(device, code(b"nsrt"), GLOBAL, 0).unwrap_or(0.0),
        device: read::<u32>(device, code(b"ltnc"), OUTPUT, 0).unwrap_or(0),
        safety: read::<u32>(device, code(b"saft"), OUTPUT, 0).unwrap_or(0),
        buffer: read::<u32>(device, code(b"fsiz"), GLOBAL, 0).unwrap_or(0),
        stream,
    })
}

/// The default output device's volume, 0–1: the main element, else the
/// first channel's.
pub(crate) fn output_volume() -> Option<f32> {
    let device = default_device(false)?;
    let volume = code(b"volm");
    read::<f32>(device, volume, OUTPUT, 0).or_else(|| read::<f32>(device, volume, OUTPUT, 1))
}
