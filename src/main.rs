use core::panic;
use embedded_graphics::mono_font::iso_8859_10::FONT_10X20;
use embedded_graphics::{
    mono_font::MonoTextStyleBuilder,
    pixelcolor::BinaryColor,
    prelude::*,
    text::{Baseline, Text},
};
use linux_embedded_hal::I2cdev;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::Method;
use rppal::gpio::{Event, Gpio, Trigger};
use serde::Deserialize;
use serde_json::json;
use ssd1306::{prelude::*, I2CDisplayInterface, Ssd1306};
use std::{sync::mpsc, time::Duration};

struct PinEvent {
    pin: u8,
    event: Event,
}

fn main() {
    let button_handle = std::thread::spawn({
        || {
            buttons();
        }
    });

    let screen_handle = std::thread::spawn({
        || {
            screen();
        }
    });

    button_handle.join().unwrap();
    screen_handle.join().unwrap();
}

fn buttons() {
    // Create a new GPIO instance
    let gpio = Gpio::new().unwrap();

    // Create a channel to receive interrupt events
    let (tx, rx) = mpsc::channel();

    // Stop InputPin from being dropped
    let mut pins = Vec::new();

    for pin_no in &[17u8, 27u8] {
        let pin_no = *pin_no;

        let tx = tx.clone();

        // Configure pin 17 as an input
        let mut pin = gpio.get(pin_no).unwrap().into_input();

        // Set up an interrupt handler for both RisingEdge and FallingEdge
        pin.set_async_interrupt(
            Trigger::Both,
            Some(Duration::from_millis(20)),
            move |event| {
                tx.send(PinEvent { pin: pin_no, event }).unwrap();
            },
        )
        .unwrap();

        pins.push(pin);

        println!("Listening for GPIO changes on pin {pin_no}...");
    }

    let client = http_client();

    // Main thread waits for GPIO events
    for msg in rx {
        let PinEvent { pin, event } = msg;

        match event.trigger {
            Trigger::RisingEdge => {
                println!("{pin} pressed");

                if pin == 17 {
                    consume_product(&client, 62);
                } else if pin == 27 {
                    add_product(&client, 62);
                } else {
                    panic!("unknown pin {pin}");
                }
            }
            _ => {
                // ignore
            }
        }
    }

    println!("Exiting...");
}

#[derive(Deserialize, Debug)]
struct Product {
    stock_amount: i32,
}

fn screen() {
    let i2c = I2cdev::new("/dev/i2c-1").unwrap();

    let interface = I2CDisplayInterface::new(i2c);
    let mut disp = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    disp.init().unwrap();

    let text_style = MonoTextStyleBuilder::new()
        .font(&FONT_10X20)
        .text_color(BinaryColor::On)
        .build();

    let client = http_client();

    loop {
        disp.clear();

        let product = get_product(&client, 62);

        Text::with_baseline("Toilet Rolls", Point::new(2, 1), text_style, Baseline::Top)
            .draw(&mut disp)
            .unwrap();

        Text::with_baseline(
            &format!("{}", &product.stock_amount),
            Point::new(2, 20),
            text_style,
            Baseline::Top,
        )
        .draw(&mut disp)
        .unwrap();

        disp.flush().unwrap();

        std::thread::sleep(Duration::from_millis(500));
    }
}

fn http_client() -> reqwest::blocking::Client {
    let mut headers = HeaderMap::new();
    headers.insert(
        "GROCY-API-KEY",
        HeaderValue::from_str("G9owQF7bLWDKctBYB6WTZ0xrndSMTLumHJ3k8WtitNozAL9C81").unwrap(),
    );
    headers.insert("accept", HeaderValue::from_str("application/json").unwrap());

    reqwest::blocking::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap()
}

fn get_product(client: &reqwest::blocking::Client, id: i32) -> Product {
    let req = reqwest::blocking::Request::new(
        Method::GET,
        format!("http://100.117.133.36:9283/api/stock/products/{}", id)
            .parse()
            .unwrap(),
    );
    let res: Product = client.execute(req).unwrap().json().unwrap();
    res
}

fn consume_product(client: &reqwest::blocking::Client, id: i32) {
    let res = client
        .post(format!(
            "http://100.117.133.36:9283/api/stock/products/{}/consume",
            id
        ))
        .json(&json!({
            "amount": 1,
            "transaction_type": "consume",
            "spoiled": false,
        }))
        .send()
        .unwrap();

    if !res.status().is_success() {
        panic!("req failed");
    }
}

fn add_product(client: &reqwest::blocking::Client, id: i32) {
    let res = client
        .post(format!(
            "http://100.117.133.36:9283/api/stock/products/{}/add",
            id
        ))
        .json(&json!({
            "amount": 1,
            "transaction_type": "purchase",
            "best_before_date": None::<String>,
            "price": None::<String>,
        }))
        .send()
        .unwrap();

    if !res.status().is_success() {
        panic!("req failed");
    }
}
