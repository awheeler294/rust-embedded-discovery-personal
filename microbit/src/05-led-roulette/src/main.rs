#![deny(unsafe_code)]
#![no_main]
#![no_std]

use cortex_m_rt::entry;
use rtt_target::{rtt_init_print, rprintln};
use panic_rtt_target as _;
use microbit::{
    board::Board,
    display::blocking::Display,
    hal::{prelude::*, Timer},
};

#[entry]
fn main() -> ! {
    rtt_init_print!();
    let mut board = Board::take().unwrap();

    let mut timer = Timer::new(board.TIMER0);
    let mut display = Display::new(board.display_pins);
    let mut display_matrix = [
        [0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0],
    ];

    let mut x = 0;
    let mut dx = 1_isize;
    let mut y = 0;
    let mut dy = 0_isize;
    loop {
        display_matrix[y][x] = 1;
        // Show light_it_all for 1000ms
        display.show(&mut timer, display_matrix, 30);
        display_matrix[y][x] = 0;

        (x, y) = next_xy(x, y);

    }

    fn next_xy(x: usize, y: usize) -> (usize, usize) {
        if y == 0 {
            if x < 4 {
                return (x + 1, y);
            }
            else {
                return (x, y + 1);
            }
        }
        else if x == 4 {
            if y < 4 {
                return (x, y + 1);
            }
            else {
                return (x - 1, y);
            }
        }
        else if y == 4 {
            if x > 0 {
                return (x - 1, y);
            }
            else {
                return (x, y - 1);
            }

        }
        else if x == 0 {
            if y > 0 {
                return (x, y -1);
            }
            else {
                return (x + 1, y);
            }

        }
        (0, 0)
    }
}
