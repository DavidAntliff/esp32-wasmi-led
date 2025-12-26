#![no_std]

// External function provided by the host
extern "C" {
    fn output(num: u64);
}

extern "C" {
    fn set_pixel(x: u32, y: u32, val: u32);
}

// Don't call the entry 'main' as it will get wrapped with C-style (argc, argv) parameters

#[no_mangle]
pub extern "C" fn fib(mut count: u64) -> u64 {
    let mut a: u64 = 0;
    let mut b: u64 = 1;

    unsafe {
        output(a);
        output(b);
    }
    
    while count > 0 {
        let next = a.wrapping_add(b);
        unsafe {
            output(next);
        }
        
        a = b;
        b = next;

        // Prevent overflow by resetting when numbers get too large
        if next > 1_000_000_000_000_000_000 {
            a = 0;
            b = 1;
        }

        count -= 1;
    }

    b
}

#[no_mangle]
pub extern "C" fn add(x: i32, y: i32) -> i32 {
    x + y
}

#[no_mangle]
pub extern "C" fn fill(max_x: u32, max_y: u32, val: u32) {
    for x in 0..max_x {
        for y in 0..max_y {
            //let val = val.wrapping_add((x as u32).wrapping_mul(31)).wrapping_add(y as u32);
            unsafe {
                set_pixel(x, y, val);
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn render(max_x: u32, max_y: u32, frames: u32) {
    let mut val = 0;
    for f in 0..frames {
        val += f;
        fill(max_x, max_y, val);
    }
}

// Panic handler required for no_std
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
