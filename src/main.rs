use anyhow::anyhow;
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
    let (err_tx, err_rx) = mpsc::channel();

    for panel in panels {
        handles.push(std::thread::spawn({
            let err_tx = err_tx.clone();
            move || {
                if let Err(e) = buttons(panel.product_id, panel.consume_pin, panel.add_pin) {
                    err_tx.send(e)
                } else {
                    Ok(())
                }
            }
        }));

        handles.push(std::thread::spawn({
            let err_tx = err_tx.clone();
            move || {
                if let Err(e) = screen(panel.device, panel.product_id) {
                    err_tx.send(e)
                } else {
                    Ok(())
                }
            }
        }));
    }

    if let Some(e) = err_rx.iter().next() {
        panic!("{:?}", e);
    }

    for handle in handles {
        handle.join().unwrap().unwrap();
    }
}

fn buttons(product_id: i32, consume_pin: u8, add_pin: u8) -> anyhow::Result<()> {
    let gpio = Gpio::new()?;

    // Create a channel to receive interrupt events
    let (tx, rx) = mpsc::channel();

    // Stop InputPin from being dropped
    let mut pins = Vec::new();

    for pin_no in &[consume_pin, add_pin] {
        let pin_no = *pin_no;

        let tx = tx.clone();

        let mut pin = gpio.get(pin_no)?.into_input();

        pin.set_async_interrupt(
            Trigger::Both,
            Some(Duration::from_millis(20)),
            move |event| {
                if let Err(e) = tx.send(PinEvent { pin: pin_no, event }) {
                    println!("unable to send pin event from {pin_no}: {e:?}");
                };
            },
        )?;

        pins.push(pin);

        println!("Listening for GPIO changes on pin {pin_no}...");
    }

    let client = http_client()?;

    for msg in rx {
        let PinEvent { pin, event } = msg;

        match event.trigger {
            Trigger::RisingEdge => {
                println!("{pin} pressed");

                if pin == consume_pin {
                    consume_product(&client, product_id)?;
                } else if pin == add_pin {
                    add_product(&client, product_id)?;
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
    Ok(())
}

fn screen(path: &str, product_id: i32) -> anyhow::Result<()> {
    let i2c = I2cdev::new(path)?;

    let interface = I2CDisplayInterface::new(i2c);
    let mut disp = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();

    disp.init()
        .map_err(|e| anyhow!("unable to flush: {:?}", e))?;

    let text_style = MonoTextStyleBuilder::new()
        .font(&FONT_10X20)
        .text_color(BinaryColor::On)
        .build();

    let client = http_client()?;

    loop {
        disp.clear();

        let product = get_product(&client, product_id)?;

        Text::with_baseline(
            &product.product.name,
            Point::new(2, 1),
            text_style,
            Baseline::Top,
        )
        .draw(&mut disp)
        .map_err(|e| anyhow!("unable to flush: {:?}", e))?;

        Text::with_baseline(
            &format!("{}", &product.stock_amount),
            Point::new(2, 20),
            text_style,
            Baseline::Top,
        )
        .draw(&mut disp)
        .map_err(|e| anyhow!("unable to flush: {:?}", e))?;

        disp.flush()
            .map_err(|e| anyhow!("unable to flush: {:?}", e))?;

        std::thread::sleep(Duration::from_millis(500));
    }
}

pub fn http_client() -> anyhow::Result<reqwest::blocking::Client> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "GROCY-API-KEY",
        /* good luck */
        HeaderValue::from_str("G9owQF7bLWDKctBYB6WTZ0xrndSMTLumHJ3k8WtitNozAL9C81")?,
    );
    headers.insert("accept", HeaderValue::from_str("application/json")?);

    Ok(reqwest::blocking::Client::builder()
        .default_headers(headers)
        .build()?)
}

pub fn get_product(client: &reqwest::blocking::Client, id: i32) -> anyhow::Result<Product> {
    let req = reqwest::blocking::Request::new(
        Method::GET,
        format!("http://100.117.133.36:9283/api/stock/products/{}", id).parse()?,
    );
    let res: Product = client.execute(req)?.json()?;
    Ok(res)
}

fn consume_product(client: &reqwest::blocking::Client, id: i32) -> anyhow::Result<()> {
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
        .send()?;

    if !res.status().is_success() {
        return Err(anyhow!("bad status: {}", res.status()));
    }

    Ok(())
}

fn add_product(client: &reqwest::blocking::Client, id: i32) -> anyhow::Result<()> {
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
        .send()?;

    if !res.status().is_success() {
        return Err(anyhow!("bad status: {}", res.status()));
    }

    Ok(())
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
