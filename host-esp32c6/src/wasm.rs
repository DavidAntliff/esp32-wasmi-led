use crate::{FRAME_CONSUMED, FRAME_LEN, FRAME_PTR, FRAME_READY, HOST_BUFFER_PTR, MODE, Mode, log};
use common::LED_BUFFER_SIZE;
use core::sync::atomic::Ordering;
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer};
use esp_hal::time::Instant;
use wasmi::{Engine, Linker, Memory, Module, Store, TypedFunc};

const TICKS_PER_SECOND: u64 = 256;

pub struct AppState {
    start_time: Instant,
    ticks: u64,
    counter: u64,
}

pub struct GuestState {
    _engine: Engine,
    store: Store<()>,
    _linker: Linker<()>,
    memory: Memory,
    host_buffer_offset: u32,

    // Guest exports
    _init: TypedFunc<(), ()>,
    update: TypedFunc<(u64, u64, u32), u32>,
}

#[embassy_executor::task]
pub async fn wasm_task() {
    log!("🌱 Start WASM task...");

    let wasm_bytes = include_bytes!("../../target/wasm32-unknown-unknown/release/guest.wasm");
    log!("⚙️ Initialising WASMI engine...");
    let engine = Engine::default();
    log!("⚙️ Initialising WASMI module...");
    let module = Module::new(&engine, wasm_bytes).expect("Failed to create module");
    log!("⚙️ Initialising WASMI store...");
    let mut store = Store::new(&engine, ());
    log!("⚙️ Initialising WASMI linker...");
    let linker = Linker::<()>::new(&engine);

    log!("⚙️ Instantiating WASMI instance...");
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .expect("Failed to instantiate module");

    let memory = instance
        .get_memory(&store, "memory")
        .expect("Failed to get guest memory");

    let host_buffer_offset = memory.data(&store).len() as u32;

    // Grow guest memory by 1 page (64KiB) to give some space for the host buffer
    memory.grow(&mut store, 1).expect("Failed to grow memory");
    log!(
        "⚙️ Guest memory size: 0x{:04x} bytes @ offset 0x{:04x}",
        memory.data(&store).len(),
        host_buffer_offset
    );

    assert!(
        host_buffer_offset as usize + LED_BUFFER_SIZE <= (memory.data_size(&store)),
        "Not enough memory for host pixel buffer"
    );

    // Store the host buffer pointer for sharing between tasks
    let host_buffer_ptr = memory.data(&store).as_ptr() as usize + host_buffer_offset as usize;
    HOST_BUFFER_PTR.store(host_buffer_ptr, Ordering::Release);

    let update_func = instance
        .get_typed_func::<(u64, u64, u32), u32>(&mut store, "update")
        .expect("Failed to get 'update' function");

    let init_func = instance
        .get_typed_func::<(), ()>(&mut store, "init")
        .expect("Failed to get 'init' function");

    log!("🧳 Calling guest 'init' function...");
    init_func
        .call(&mut store, ())
        .expect("Failed to call guest 'init' function");

    let mut guest_state = GuestState {
        _engine: engine,
        store,
        _linker: linker,
        memory,
        host_buffer_offset,
        _init: init_func,
        update: update_func,
    };

    let mut app_state = AppState {
        start_time: Instant::now(),
        ticks: 0,
        counter: 0,
    };

    let mut current_mode = Mode::default();

    let mut receiver = MODE.receiver().unwrap();

    log!("🔁 WASMI entering main loop...");

    loop {
        match select(receiver.changed(), Timer::after(Duration::from_millis(1))).await {
            Either::First(mode) => {
                current_mode = mode;
            }
            Either::Second(_) => {
                if current_mode != Mode::Wasm {
                    continue;
                }

                let elapsed = Instant::now() - app_state.start_time;
                app_state.ticks = elapsed.as_millis() * TICKS_PER_SECOND / 1000;

                let pixel_buffer = guest_state
                    .update
                    .call(
                        &mut guest_state.store,
                        (
                            app_state.ticks,
                            app_state.counter,
                            guest_state.host_buffer_offset,
                        ),
                    )
                    .expect("Failed to call 'update' function");

                // Check mode wasn't changed while guest was executing
                if let Some(mode) = receiver.try_changed() {
                    current_mode = mode;
                    if current_mode != Mode::Wasm {
                        continue; // discard this frame
                    }
                }

                // Get a raw pointer to the pixel data inside WASM linear memory
                let mem_data = guest_state.memory.data(&guest_state.store);
                let offset = pixel_buffer as usize;
                let len = LED_BUFFER_SIZE;
                assert!(offset + len <= mem_data.len(), "pixel buffer out of bounds");

                let ptr = mem_data[offset..].as_ptr() as usize;

                // Publish the pointer — safe because led_task won't read until signalled,
                // and we block until it's done.
                FRAME_PTR.store(ptr, Ordering::Release);
                FRAME_LEN.store(len, Ordering::Release);

                FRAME_READY.signal(());
                FRAME_CONSUMED.wait().await;

                app_state.counter += 1;
            }
        }
    }
}
