#![no_main]
#![no_std]

use core::fmt::Write;
use core::{
    borrow::Borrow,
    cell::{Cell, RefCell},
};
use cortex_m::interrupt::Mutex;
use cortex_m::peripheral::Peripherals;
use cortex_m_rt::entry;
use heapless::Vec;
use panic_rtt_target as _;
use rtt_target::{rprintln, rtt_init_print};

use microbit::{
    board::Board,
    display::nonblocking::{Display, GreyscaleImage},
    hal::{
        clocks::Clocks,
        prelude::*,
        rtc::{Rtc, RtcInterrupt},
        Timer,
    },
    pac::{self, interrupt, RTC0, TIMER1},
};

#[cfg(feature = "v1")]
use microbit::{
    hal::prelude::*,
    hal::uart,
    hal::uart::{Baudrate, Parity},
};

#[cfg(feature = "v2")]
use microbit::{
    hal::prelude::*,
    hal::uarte,
    hal::uarte::{Baudrate, Parity},
};

#[cfg(feature = "v2")]
mod serial_setup;
#[cfg(feature = "v2")]
use serial_setup::UartePort;

// We use TIMER1 to drive the display, and RTC0 to update the animation.
// We set the TIMER1 interrupt to a higher priority than RTC0.

static DISPLAY: Mutex<RefCell<Option<Display<TIMER1>>>> = Mutex::new(RefCell::new(None));
static ANIM_TIMER: Mutex<RefCell<Option<Rtc<RTC0>>>> = Mutex::new(RefCell::new(None));
static DISPLAY_CH: Mutex<Cell<Option<u8>>> = Mutex::new(Cell::new(None));

const ENTER: char = '\r';
const BACKSPACE: char = '\x08';
const SPACE: char = '\x20';

#[entry]
fn main() -> ! {
    rtt_init_print!();
    let mut board = microbit::Board::take().unwrap();

    // Starting the low-frequency clock (needed for RTC to work)
    Clocks::new(board.CLOCK).start_lfclk();

    // RTC at 16Hz (32_768 / (2047 + 1))
    // 62.5ms period
    let mut rtc0 = Rtc::new(board.RTC0, 2047).unwrap();
    rtc0.enable_event(RtcInterrupt::Tick);
    rtc0.enable_interrupt(RtcInterrupt::Tick, None);
    rtc0.enable_counter();

    // Create display
    let display = Display::new(board.TIMER1, board.display_pins);

    cortex_m::interrupt::free(move |cs| {
        *DISPLAY.borrow(cs).borrow_mut() = Some(display);
        *ANIM_TIMER.borrow(cs).borrow_mut() = Some(rtc0);
    });
    unsafe {
        board.NVIC.set_priority(pac::Interrupt::RTC0, 64);
        board.NVIC.set_priority(pac::Interrupt::TIMER1, 128);
        pac::NVIC::unmask(pac::Interrupt::RTC0);
        pac::NVIC::unmask(pac::Interrupt::TIMER1);
    }

    //let mut timer = Timer::new(board.TIMER0);
    //let mut display = Display::new(board.display_pins);

    #[cfg(feature = "v1")]
    let mut serial = {
        uart::Uart::new(
            board.UART0,
            board.uart.into(),
            Parity::EXCLUDED,
            Baudrate::BAUD115200,
        )
    };

    #[cfg(feature = "v2")]
    let mut serial = {
        let serial = uarte::Uarte::new(
            board.UARTE0,
            board.uart.into(),
            Parity::EXCLUDED,
            Baudrate::BAUD115200,
        );
        UartePort::new(serial)
    };

    write!(serial, "Type Something.\r\n").unwrap();
    nb::block!(serial.flush()).unwrap();

    // A buffer with 32 bytes of capacity
    let mut buffer: Vec<u8, 32> = Vec::new();
    loop {
        let byte = nb::block!(serial.read()).unwrap();

        cortex_m::interrupt::free(|cs| DISPLAY_CH.borrow(cs).set(Some(byte)));

        rprintln!("{}", byte);

        if byte == ENTER as u8 {
            write!(serial, "\r\n").unwrap();
            for ch in buffer.iter().rev().chain(&[b'\n', b'\r']) {
                nb::block!(serial.write(*ch)).unwrap();
            }
            buffer.clear();
        } else if byte == BACKSPACE as u8 {
            if let Some(_) = buffer.pop() {
                nb::block!(serial.write(byte)).unwrap();
                nb::block!(serial.write(' ' as u8)).unwrap();
                nb::block!(serial.write(byte)).unwrap();
            }
        } else {
            match buffer.push(byte) {
                Ok(_) => {
                    nb::block!(serial.write(byte)).unwrap();

                    //let display_matrix = ch_to_matrix(byte);
                    //display.show(&mut timer, display_matrix, 500);
                }
                Err(e) => {
                    rprintln!(
                        "Error appending {:#?}, buffer len: {}, max len: {}, err: {}",
                        char::from(byte),
                        buffer.len(),
                        32,
                        e
                    );
                }
            }
        }

        nb::block!(serial.flush()).unwrap();
    }
}

#[interrupt]
fn TIMER1() {
    cortex_m::interrupt::free(|cs| {
        if let Some(display) = DISPLAY.borrow(cs).borrow_mut().as_mut() {
            display.handle_display_event();
        }
    });
}

// When a character is typed in the serial console display that character on the
// LED matrix, then fade out over time.
const MAX_STEP: u8 = 24;
const MIN_STEP: u8 = 3;
const BLANK_MATRIX: GreyscaleImage = GreyscaleImage::new(&[
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
]);
#[interrupt]
unsafe fn RTC0() {
    static mut STEP: u8 = MAX_STEP;
    static mut CH: Option<u8> = None;

    cortex_m::interrupt::free(|cs| {
        if let Some(rtc) = ANIM_TIMER.borrow(cs).borrow_mut().as_mut() {
            rtc.reset_event(RtcInterrupt::Tick);
        }
    });

    let mut input_ch = None;

    cortex_m::interrupt::free(|cs| {
        if let Some(display_ch) = DISPLAY_CH.borrow(cs).get() {
            input_ch = Some(display_ch);
            *STEP = MAX_STEP;
            DISPLAY_CH.borrow(cs).set(None);
            rprintln!("display_ch {}", display_ch);
        }
    });

    if *STEP <= MIN_STEP {
        *STEP = MAX_STEP;
        *CH = None;
    };

    let brightness = match *STEP {
        0..=8 => 9 - (9 - *STEP),
        9..=MAX_STEP => 9,
        _ => unreachable!(),
    };

    let image = {
        // if the same character is typed twice blank the matrix for one tick
        if input_ch == *CH {
            BLANK_MATRIX
        } else {
            if input_ch.is_some() {
                *CH = input_ch;
            }
            ch_to_matrix(*CH, brightness)
        }
    };

    *STEP -= 1;

    cortex_m::interrupt::free(|cs| {
        if let Some(display) = DISPLAY.borrow(cs).borrow_mut().as_mut() {
            display.show(&image);
        }
    });
}

fn ch_to_matrix(ch: Option<u8>, brightness: u8) -> GreyscaleImage {
    match ch {
        None => return BLANK_MATRIX,
        Some(ch) => {
            let b = brightness;
            match char::from(ch) {
                'A' => GreyscaleImage::new(&[
                    [0, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [b, b, b, b, b],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                ]),
                'a' => GreyscaleImage::new(&[
                    [b, b, b, b, 0],
                    [0, 0, 0, 0, b],
                    [0, b, b, b, b],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, b],
                ]),
                'B' => GreyscaleImage::new(&[
                    [b, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [b, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [b, b, b, b, 0],
                ]),
                'b' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, 0],
                    [b, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [b, b, b, b, 0],
                ]),
                'C' => GreyscaleImage::new(&[
                    [0, b, b, b, b],
                    [b, 0, 0, 0, 0],
                    [b, 0, 0, 0, 0],
                    [b, 0, 0, 0, 0],
                    [0, b, b, b, b],
                ]),
                'c' => GreyscaleImage::new(&[
                    [0, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, 0],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, 0],
                ]),
                'D' => GreyscaleImage::new(&[
                    [b, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [b, b, b, b, 0],
                ]),
                'd' => GreyscaleImage::new(&[
                    [0, 0, 0, 0, b],
                    [0, b, b, b, b],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, b],
                ]),
                'E' => GreyscaleImage::new(&[
                    [b, b, b, b, b],
                    [b, 0, 0, 0, 0],
                    [b, b, b, b, b],
                    [b, 0, 0, 0, 0],
                    [b, b, b, b, b],
                ]),
                'e' => GreyscaleImage::new(&[
                    [0, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [b, b, b, b, b],
                    [b, 0, 0, 0, 0],
                    [0, b, b, b, b],
                ]),
                'F' => GreyscaleImage::new(&[
                    [b, b, b, b, b],
                    [b, 0, 0, 0, 0],
                    [b, b, b, 0, 0],
                    [b, 0, 0, 0, 0],
                    [b, 0, 0, 0, 0],
                ]),
                'f' => GreyscaleImage::new(&[
                    [0, b, b, b, b],
                    [b, 0, 0, 0, 0],
                    [b, b, b, 0, 0],
                    [b, 0, 0, 0, 0],
                    [b, 0, 0, 0, 0],
                ]),
                'G' => GreyscaleImage::new(&[
                    [0, b, b, b, b],
                    [b, 0, 0, 0, 0],
                    [b, 0, b, b, b],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, b],
                ]),
                'g' => GreyscaleImage::new(&[
                    [0, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, b],
                    [0, 0, 0, 0, b],
                    [b, b, b, b, 0],
                ]),
                'H' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [b, b, b, b, b],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                ]),
                'h' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, 0],
                    [b, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                ]),
                'I' => GreyscaleImage::new(&[
                    [0, b, b, b, 0],
                    [0, 0, b, 0, 0],
                    [0, 0, b, 0, 0],
                    [0, 0, b, 0, 0],
                    [0, b, b, b, 0],
                ]),
                'i' => GreyscaleImage::new(&[
                    [0, 0, b, 0, 0],
                    [0, 0, 0, 0, 0],
                    [0, 0, b, 0, 0],
                    [0, 0, b, 0, 0],
                    [0, 0, b, 0, 0],
                ]),
                'J' => GreyscaleImage::new(&[
                    [0, 0, b, b, b],
                    [0, 0, 0, 0, b],
                    [0, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, 0],
                ]),
                'j' => GreyscaleImage::new(&[
                    [0, 0, 0, 0, b],
                    [0, 0, 0, 0, b],
                    [0, 0, 0, 0, b],
                    [0, 0, 0, 0, b],
                    [b, b, b, b, 0],
                ]),
                'K' => GreyscaleImage::new(&[
                    [b, 0, 0, b, 0],
                    [b, 0, b, 0, 0],
                    [b, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                ]),
                'k' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, b],
                    [b, 0, 0, b, 0],
                    [b, b, b, 0, 0],
                    [b, 0, 0, b, 0],
                    [b, 0, 0, 0, b],
                ]),
                'L' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, 0],
                    [b, 0, 0, 0, 0],
                    [b, 0, 0, 0, 0],
                    [b, 0, 0, 0, 0],
                    [b, b, b, b, b],
                ]),
                'l' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, 0],
                    [b, 0, 0, 0, 0],
                    [b, 0, 0, 0, 0],
                    [b, 0, 0, 0, 0],
                    [0, b, b, b, b],
                ]),
                'M' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, b],
                    [b, b, 0, b, b],
                    [b, 0, b, 0, b],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                ]),
                'm' => GreyscaleImage::new(&[
                    [0, b, 0, b, 0],
                    [b, 0, b, 0, b],
                    [b, 0, b, 0, b],
                    [b, 0, b, 0, b],
                    [b, 0, b, 0, b],
                ]),
                'N' => GreyscaleImage::new(&[
                    [b, b, 0, 0, b],
                    [b, 0, b, 0, b],
                    [b, 0, b, 0, b],
                    [b, 0, b, 0, b],
                    [b, 0, 0, b, b],
                ]),
                'n' => GreyscaleImage::new(&[
                    [b, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                ]),
                'O' => GreyscaleImage::new(&[
                    [0, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, 0],
                ]),
                'o' => GreyscaleImage::new(&[
                    [0, 0, 0, 0, 0],
                    [0, b, b, 0, 0],
                    [b, 0, 0, b, 0],
                    [b, 0, 0, b, 0],
                    [0, b, b, 0, 0],
                ]),
                'P' => GreyscaleImage::new(&[
                    [b, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [b, b, b, b, 0],
                    [b, 0, 0, 0, 0],
                    [b, 0, 0, 0, 0],
                ]),
                'p' => GreyscaleImage::new(&[
                    [b, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [b, b, b, b, 0],
                    [b, 0, 0, 0, 0],
                ]),
                'Q' => GreyscaleImage::new(&[
                    [0, b, b, 0, 0],
                    [b, 0, 0, b, 0],
                    [b, 0, 0, b, 0],
                    [b, 0, 0, b, 0],
                    [0, b, b, b, b],
                ]),
                'q' => GreyscaleImage::new(&[
                    [0, b, b, b, b],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, b],
                    [0, 0, 0, 0, b],
                ]),
                'R' => GreyscaleImage::new(&[
                    [b, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [b, b, b, b, 0],
                    [b, 0, 0, b, 0],
                    [b, 0, 0, 0, b],
                ]),
                'r' => GreyscaleImage::new(&[
                    [b, 0, b, b, 0],
                    [b, b, 0, 0, b],
                    [b, 0, 0, 0, 0],
                    [b, 0, 0, 0, 0],
                    [b, 0, 0, 0, 0],
                ]),
                'S' => GreyscaleImage::new(&[
                    [0, b, b, b, b],
                    [b, 0, 0, 0, 0],
                    [0, b, b, b, 0],
                    [0, 0, 0, 0, b],
                    [b, b, b, b, 0],
                ]),
                's' => GreyscaleImage::new(&[
                    [0, 0, b, b, 0],
                    [0, b, 0, 0, 0],
                    [0, 0, b, 0, 0],
                    [0, 0, 0, b, 0],
                    [0, b, b, 0, 0],
                ]),
                'T' => GreyscaleImage::new(&[
                    [b, b, b, b, b],
                    [0, 0, b, 0, 0],
                    [0, 0, b, 0, 0],
                    [0, 0, b, 0, 0],
                    [0, 0, b, 0, 0],
                ]),
                't' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, 0],
                    [b, b, b, 0, 0],
                    [b, 0, 0, 0, 0],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, 0],
                ]),
                'U' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, 0],
                ]),
                'u' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, b, b],
                    [0, b, b, 0, b],
                ]),
                'V' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [0, b, 0, b, 0],
                    [0, b, 0, b, 0],
                    [0, 0, b, 0, 0],
                ]),
                'v' => GreyscaleImage::new(&[
                    [0, 0, 0, 0, 0],
                    [b, 0, 0, 0, b],
                    [0, b, 0, b, 0],
                    [0, 0, b, 0, 0],
                    [0, 0, 0, 0, 0],
                ]),
                'W' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [b, 0, b, 0, b],
                    [b, b, 0, b, b],
                    [b, 0, 0, 0, b],
                ]),
                'w' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, b],
                    [b, 0, b, 0, b],
                    [b, 0, b, 0, b],
                    [b, 0, b, 0, b],
                    [0, b, 0, b, 0],
                ]),
                'X' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, b],
                    [0, b, 0, b, 0],
                    [0, 0, b, 0, 0],
                    [0, b, 0, b, 0],
                    [b, 0, 0, 0, b],
                ]),
                'x' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                ]),
                'Y' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, 0],
                    [0, 0, b, 0, 0],
                    [0, 0, b, 0, 0],
                ]),
                'y' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, b],
                    [0, 0, 0, 0, b],
                    [b, b, b, b, 0],
                ]),
                'Z' => GreyscaleImage::new(&[
                    [b, b, b, b, b],
                    [0, 0, 0, 0, b],
                    [0, b, b, b, 0],
                    [b, 0, 0, 0, 0],
                    [b, b, b, b, b],
                ]),
                'z' => GreyscaleImage::new(&[
                    [b, b, b, b, b],
                    [0, 0, 0, b, 0],
                    [0, 0, b, 0, 0],
                    [0, b, 0, 0, 0],
                    [b, b, b, b, b],
                ]),
                '0' => GreyscaleImage::new(&[
                    [0, b, b, b, 0],
                    [b, 0, 0, b, b],
                    [b, 0, b, 0, b],
                    [b, b, 0, 0, b],
                    [0, b, b, b, 0],
                ]),
                '1' => GreyscaleImage::new(&[
                    [0, 0, b, 0, 0],
                    [0, b, b, 0, 0],
                    [0, 0, b, 0, 0],
                    [0, 0, b, 0, 0],
                    [0, b, b, b, 0],
                ]),
                '2' => GreyscaleImage::new(&[
                    [b, b, b, b, 0],
                    [0, 0, 0, 0, b],
                    [0, b, b, b, 0],
                    [b, 0, 0, 0, 0],
                    [b, b, b, b, b],
                ]),
                '3' => GreyscaleImage::new(&[
                    [b, b, b, b, 0],
                    [0, 0, 0, 0, b],
                    [0, 0, b, b, 0],
                    [0, 0, 0, 0, b],
                    [b, b, b, b, b],
                ]),
                '4' => GreyscaleImage::new(&[
                    [b, 0, 0, 0, b],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, b],
                    [0, 0, 0, 0, b],
                    [0, 0, 0, 0, b],
                ]),
                '5' => GreyscaleImage::new(&[
                    [b, b, b, b, b],
                    [b, 0, 0, 0, 0],
                    [b, b, b, b, 0],
                    [0, 0, 0, 0, b],
                    [b, b, b, b, 0],
                ]),
                '6' => GreyscaleImage::new(&[
                    [0, b, b, b, b],
                    [b, 0, 0, 0, 0],
                    [b, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, 0],
                ]),
                '7' => GreyscaleImage::new(&[
                    [b, b, b, b, b],
                    [0, 0, 0, 0, b],
                    [0, 0, 0, b, 0],
                    [0, 0, 0, b, 0],
                    [0, 0, 0, b, 0],
                ]),
                '8' => GreyscaleImage::new(&[
                    [0, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, 0],
                ]),
                '9' => GreyscaleImage::new(&[
                    [0, b, b, b, 0],
                    [b, 0, 0, 0, b],
                    [0, b, b, b, b],
                    [0, 0, 0, 0, b],
                    [0, 0, b, b, 0],
                ]),
                ENTER => GreyscaleImage::new(&[
                    [0, 0, 0, 0, b],
                    [0, 0, 0, 0, b],
                    [0, b, 0, 0, b],
                    [b, b, b, b, b],
                    [0, b, 0, 0, 0],
                ]),
                BACKSPACE => GreyscaleImage::new(&[
                    [0, 0, 0, 0, 0],
                    [0, b, 0, 0, 0],
                    [b, b, b, b, b],
                    [0, b, 0, 0, 0],
                    [0, 0, 0, 0, 0],
                ]),
                ' ' => BLANK_MATRIX,
                ' ' => GreyscaleImage::new(&[
                    [0, 0, 0, 0, 0],
                    [0, 0, 0, 0, 0],
                    [0, 0, 0, 0, 0],
                    [b, 0, 0, 0, b],
                    [b, b, b, b, b],
                ]),
                // Escape
                '\x1B' => BLANK_MATRIX,
                _ => GreyscaleImage::new(&[
                    [b, b, b, b, b],
                    [b, b, b, b, b],
                    [b, b, b, b, b],
                    [b, b, b, b, b],
                    [b, b, b, b, b],
                ]),
            }
        }
    }
}
