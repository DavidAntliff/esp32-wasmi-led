use crate::{
    DIRECT_CMD, DirectCommand, FRAME_CONSUMED, FRAME_LEN, FRAME_PTR, FRAME_READY, HOST_BUFFER_PTR,
    MODE, Mode, log,
};
use common::{LED_BUFFER_SIZE, set_all, set_color};
use core::sync::atomic::Ordering;
use embassy_futures::select::{Either, select};

#[embassy_executor::task]
pub async fn direct_task() {
    log!("🌱 Start Direct task...");

    let mut current_mode = Mode::default();
    let mut receiver = MODE.receiver().unwrap();

    log!("🔁 Direct entering main loop...");
    loop {
        let active = current_mode == Mode::Direct;
        let host_pixel_ptr = HOST_BUFFER_PTR.load(Ordering::Acquire) as *mut u8;
        let host_pixel_ptr = (!host_pixel_ptr.is_null()).then_some(host_pixel_ptr);

        match select(receiver.changed(), DIRECT_CMD.receive()).await {
            Either::First(mode) => {
                current_mode = mode;

                log!("Direct mode: host_pixel_ptr {:?}", host_pixel_ptr);
            }
            Either::Second(cmd) => {
                if let Some(host_pixel_ptr) = host_pixel_ptr {
                    match cmd {
                        DirectCommand::SetPixel { point, color } => {
                            log!("SetPixel: {:?}, {:?}", point, color);

                            if active {
                                // SAFETY: host_pixel_ptr points to a valid pixel buffer
                                unsafe {
                                    set_color(
                                        host_pixel_ptr,
                                        (point.x.into(), point.y.into()),
                                        (color.r, color.g, color.b),
                                    )
                                };
                            }
                        }
                        DirectCommand::SetAll { color } => {
                            log!("SetAll: {:?}", color);

                            if active {
                                // SAFETY: host_pixel_ptr points to a valid pixel buffer
                                unsafe { set_all(host_pixel_ptr, (color.r, color.g, color.b)) };
                            }
                        }
                    }

                    if active {
                        // Publish the host buffer pointer — safe because led_task won't read
                        // until signalled, and we block until it's done.
                        FRAME_PTR.store(host_pixel_ptr as usize, Ordering::Release);
                        FRAME_LEN.store(LED_BUFFER_SIZE, Ordering::Release);

                        FRAME_READY.signal(());
                        FRAME_CONSUMED.wait().await;
                    }
                } else {
                    // host pointer isn't valid (yet)
                    continue;
                }
            }
        }
    }
}
