use std::time::Duration;

use rppal::gpio::Gpio;

fn main() {
    let gpio = Gpio::new().unwrap();

    loop {
        let mut pin = gpio.get(17).unwrap().into_input();

        if pin.is_low() {
            println!("low");
        } else if pin.is_high() {
            println!("high");
        } else {
            println!("wtf");
        }

        std::thread::sleep(Duration::from_millis(100));
    }

    
    println!("Hello, world!");
}
