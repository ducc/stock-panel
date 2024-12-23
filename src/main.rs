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

struct StockPanel<'a> {
    product_id: i32,
    consume_pin: u8,
    add_pin: u8,
    device: &'a str,
}

fn main() {
    let panels = &[
        StockPanel {
            // Toilet Rolls
            product_id: 62,
            consume_pin: 9,
            add_pin: 10,
            device: "/dev/i2c-0",
        },
        StockPanel {
            // Onions
            product_id: 43,
            consume_pin: 17,
            add_pin: 27,
            device: "/dev/i2c-1",
        },
    ];

    let mut handles = vec![];

    for panel in panels {
        handles.push(std::thread::spawn(|| {
            buttons(panel.product_id, panel.consume_pin, panel.add_pin)
        }));
        handles.push(std::thread::spawn(|| {
            screen(panel.device, panel.product_id)
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }
}

fn buttons(product_id: i32, consume_pin: u8, add_pin: u8) {
    let gpio = Gpio::new().unwrap();

    // Create a channel to receive interrupt events
    let (tx, rx) = mpsc::channel();

    // Stop InputPin from being dropped
    let mut pins = Vec::new();

    for pin_no in &[consume_pin, add_pin] {
        let pin_no = *pin_no;

        let tx = tx.clone();

        let mut pin = gpio.get(pin_no).unwrap().into_input();

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

    for msg in rx {
        let PinEvent { pin, event } = msg;

        match event.trigger {
            Trigger::RisingEdge => {
                println!("{pin} pressed");

                if pin == consume_pin {
                    consume_product(&client, product_id);
                } else if pin == add_pin {
                    add_product(&client, product_id);
                } else {
                    println!("unknown pin {pin}");
                }
            }
            _ => {
                // ignore
            }
        }
    }

    println!("Exiting...");
}

fn screen(path: &str, product_id: i32) {
    let i2c = I2cdev::new(path).unwrap();

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

        let product = get_product(&client, product_id);

        Text::with_baseline(
            &product.product.name,
            Point::new(2, 1),
            text_style,
            Baseline::Top,
        )
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

pub fn http_client() -> reqwest::blocking::Client {
    let mut headers = HeaderMap::new();
    headers.insert(
        "GROCY-API-KEY",
        /* good luck */
        HeaderValue::from_str("G9owQF7bLWDKctBYB6WTZ0xrndSMTLumHJ3k8WtitNozAL9C81").unwrap(),
    );
    headers.insert("accept", HeaderValue::from_str("application/json").unwrap());

    reqwest::blocking::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap()
}

pub fn get_product(client: &reqwest::blocking::Client, id: i32) -> Product {
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

struct PinEvent {
    pin: u8,
    event: Event,
}

#[derive(Deserialize, Debug)]
pub struct Product {
    pub stock_amount: i32,
    pub product: ProductDetails,
}

#[derive(Deserialize, Debug)]
pub struct ProductDetails {
    pub name: String,
}
