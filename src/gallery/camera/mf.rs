//! The camera on Windows, through Media Foundation: the video capture devices it lists, read with
//! a source reader that converts their pictures (often YUV or JPEG) to 32-bit RGB, as the player's
//! frames are (see `player::mf`).

use std::ffi::c_void;
use std::ptr::null_mut;

use eframe::egui::ColorImage;
use windows::core::{GUID, PWSTR};
use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

use super::{Sink, SIDE};
use crate::gallery::platform::SoundDevice;

/// A camera Media Foundation lists: how to start it, and its id and name.
struct Device {
    activate: IMFActivate,
    /// Its symbolic link, which Settings keeps.
    link: String,
    name: String,
}

/// The cameras there are, to choose from.
pub fn cameras() -> Vec<SoundDevice> {
    unsafe { devices() }.unwrap_or_default().into_iter().map(|d| SoundDevice { id: d.link, name: d.name }).collect()
}

unsafe fn devices() -> windows::core::Result<Vec<Device>> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
    let mut attrs = None;
    MFCreateAttributes(&mut attrs, 1)?;
    let attrs = attrs.unwrap();
    attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)?;
    let (mut list, mut count): (*mut Option<IMFActivate>, u32) = (null_mut(), 0);
    MFEnumDeviceSources(&attrs, &mut list, &mut count)?;
    let mut devices = Vec::new();
    if list.is_null() {
        return Ok(devices);
    }
    for i in 0..count as usize {
        // Taken out of the list, which is freed below; each is released when its `Device` goes.
        if let Some(activate) = (*list.add(i)).take() {
            let link = text(&activate, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK).unwrap_or_default();
            let name = text(&activate, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME).unwrap_or_else(|| "Camera".into());
            devices.push(Device { activate, link, name });
        }
    }
    CoTaskMemFree(Some(list as *const c_void));
    Ok(devices)
}

/// One of a device's text attributes.
unsafe fn text(activate: &IMFActivate, key: &GUID) -> Option<String> {
    let (mut value, mut len) = (PWSTR::null(), 0u32);
    activate.GetAllocatedString(key, &mut value, &mut len).ok()?;
    let text = value.to_string().ok();
    CoTaskMemFree(Some(value.0 as *const c_void));
    text
}

/// Why the camera can't be used, in a few words.
fn why(e: &windows::core::Error) -> String {
    match e.code() {
        E_ACCESSDENIED => "Windows' privacy settings don't let apps use it: Settings → Privacy & security → Camera".into(),
        MF_E_HW_MFT_FAILED_START_STREAMING | MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED => "it's in use by another app".into(),
        MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED => "it was disconnected".into(),
        code => match e.message().trim().trim_end_matches('.').to_lowercase() {
            message if message.is_empty() => format!("error {:#010x}", code.0),
            message => message,
        },
    }
}

/// Sends `device`'s pictures (the first camera's, if it's `None` or gone) to `sink` until nothing's
/// looking (`Ok`), or why it can't.
pub fn capture(device: Option<&str>, sink: &Sink) -> Result<(), String> {
    let devices = unsafe { devices() }.map_err(|e| why(&e))?;
    let chosen = device.and_then(|d| devices.iter().find(|c| c.link == d));
    if device.is_some() && chosen.is_none() && !devices.is_empty() {
        sink.note("The chosen camera isn't connected, so the first one was shown.");
    }
    let camera = chosen.or(devices.first()).ok_or("no camera was found")?;
    unsafe {
        let source: IMFMediaSource = camera.activate.ActivateObject().map_err(|e| why(&e))?;
        let read = stream(&source, sink).map_err(|e| why(&e));
        let _ = source.Shutdown();
        let _ = camera.activate.ShutdownObject();
        read?
    }
}

/// Reads `source`'s pictures into `sink`: `Ok(Ok(()))` once nothing's looking, `Ok(Err(why))` if it
/// stops sending them.
unsafe fn stream(source: &IMFMediaSource, sink: &Sink) -> windows::core::Result<Result<(), String>> {
    let mut attrs = None;
    MFCreateAttributes(&mut attrs, 1)?;
    let attrs = attrs.unwrap();
    // Converts whatever the camera sends to the RGB asked for below.
    attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
    let reader = MFCreateSourceReaderFromMediaSource(source, &attrs)?;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    smallest_big_enough(&reader, stream);
    let wanted = MFCreateMediaType()?;
    wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    wanted.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
    reader.SetCurrentMediaType(stream, None, &wanted)?;
    let current = reader.GetCurrentMediaType(stream)?;
    let size = current.GetUINT64(&MF_MT_FRAME_SIZE)?;
    let (width, height) = ((size >> 32) as usize, (size & 0xFFFF_FFFF) as usize);
    // Rows run top-down for a positive stride, bottom-up for a negative one.
    let stride = current.GetUINT32(&MF_MT_DEFAULT_STRIDE).map_or(width as i32 * 4, |s| s as i32);
    loop {
        let (mut flags, mut sample) = (0u32, None);
        reader.ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample))?;
        if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 | MF_SOURCE_READERF_ERROR.0) as u32 != 0 {
            return Ok(Err("it stopped sending pictures".into()));
        }
        let Some(sample) = sample else { continue };
        let buffer = sample.ConvertToContiguousBuffer()?;
        let (mut data, mut len) = (null_mut(), 0u32);
        buffer.Lock(&mut data, None, Some(&mut len))?;
        let picture = square(std::slice::from_raw_parts(data, len as usize), width, height, stride, SIDE);
        let _ = buffer.Unlock();
        if !sink.picture(picture) {
            return Ok(Ok(()));
        }
    }
}

/// Asks for the camera's smallest picture that's still big enough for the bubble, at 15 frames a
/// second or more, rather than its default (often its biggest), which takes far more converting.
unsafe fn smallest_big_enough(reader: &IMFSourceReader, stream: u32) {
    let mut best: Option<(u64, IMFMediaType)> = None;
    for i in 0.. {
        let Ok(native) = reader.GetNativeMediaType(stream, i) else { break };
        let Ok(size) = native.GetUINT64(&MF_MT_FRAME_SIZE) else { continue };
        let (w, h) = (size >> 32, size & 0xFFFF_FFFF);
        let fps = native.GetUINT64(&MF_MT_FRAME_RATE).map_or(30.0, |r| (r >> 32) as f64 / (r & 0xFFFF_FFFF).max(1) as f64);
        if w.min(h) >= SIDE as u64 && fps >= 15.0 && best.as_ref().is_none_or(|(area, _)| w * h < *area) {
            best = Some((w * h, native));
        }
    }
    if let Some((_, native)) = best {
        let _ = reader.SetCurrentMediaType(stream, None, &native);
    }
}

/// The square from the middle of a `width` × `height` BGRX picture (rows `stride` bytes apart,
/// bottom-up if it's negative), as RGBA, made at most `max` pixels across by averaging.
fn square(bytes: &[u8], width: usize, height: usize, stride: i32, max: usize) -> ColorImage {
    let side = width.min(height);
    let step = side.div_ceil(max).max(1);
    let out = (side / step).max(1);
    let (left, top) = ((width - side) / 2, (height - side) / 2);
    let pitch = stride.unsigned_abs() as usize;
    let mut rgba = vec![0u8; out * out * 4];
    for oy in 0..out {
        for ox in 0..out {
            let mut sum = [0u32; 3];
            for y in top + oy * step..top + (oy + 1) * step {
                let row = if stride < 0 { height - 1 - y } else { y } * pitch;
                for x in left + ox * step..left + (ox + 1) * step {
                    if let Some(p) = bytes.get(row + x * 4..row + x * 4 + 3) {
                        // BGR.
                        sum = [sum[0] + p[2] as u32, sum[1] + p[1] as u32, sum[2] + p[0] as u32];
                    }
                }
            }
            let n = (step * step) as u32;
            rgba[(oy * out + ox) * 4..][..4].copy_from_slice(&[(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8, 255]);
        }
    }
    ColorImage::from_rgba_unmultiplied([out, out], &rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A BGRX picture whose every pixel says where it is: blue x, green y.
    fn picture(width: usize, height: usize) -> Vec<u8> {
        (0..height).flat_map(|y| (0..width).flat_map(move |x| [x as u8, y as u8, 200, 0])).collect()
    }

    #[test]
    fn takes_the_middle_square_as_rgba() {
        // 8 × 4: the middle 4 × 4 is x 2 to 5.
        let image = square(&picture(8, 4), 8, 4, 32, 480);
        assert_eq!(image.size, [4, 4]);
        assert_eq!(image.pixels[0].to_array(), [200, 0, 2, 255], "red from the R, its x in blue");
        assert_eq!(image.pixels[15].to_array(), [200, 3, 5, 255]);
        // Bottom-up rows: the first row is the last one in memory.
        let flipped = square(&picture(8, 4), 8, 4, -32, 480);
        assert_eq!(flipped.pixels[0].to_array(), [200, 3, 2, 255]);
    }

    #[test]
    fn shrinks_a_big_picture_by_averaging() {
        // 1280 × 720 to 360 × 360: each pixel the average of 2 × 2.
        let image = square(&picture(1280, 720), 1280, 720, 1280 * 4, 480);
        assert_eq!(image.size, [360, 360]);
        let [r, g, b, _] = image.pixels[0].to_array();
        assert_eq!((r, g, b), (200, 0, ((280 + 281) / 2 % 256) as u8), "x 280 and 281, y 0 and 1");
        // A short buffer leaves black rather than reading past its end.
        let short = square(&picture(8, 4)[..64], 8, 4, 32, 480);
        assert_eq!(short.pixels[15].to_array(), [0, 0, 0, 255]);
    }

    #[test]
    fn says_why_a_camera_cant_be_used() {
        let error = |code| windows::core::Error::from_hresult(code);
        assert_eq!(why(&error(E_ACCESSDENIED)), "Windows' privacy settings don't let apps use it: Settings → Privacy & security → Camera");
        assert_eq!(why(&error(MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED)), "it's in use by another app");
        assert_eq!(why(&error(MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED)), "it was disconnected");
    }

    /// Whatever cameras this PC has are listed with names and ids (none, on CI's machines).
    #[test]
    fn lists_the_cameras() {
        for camera in cameras() {
            assert!(!camera.name.is_empty() && !camera.id.is_empty(), "{camera:?}");
        }
    }
}
