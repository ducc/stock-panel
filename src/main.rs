use anyhow::anyhow;
use embedded_graphics::mono_font::ascii::FONT_6X10;
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
use std::sync::{Arc, Mutex};
use std::{sync::mpsc, time::Duration};

// struct StockPanel<'a> {
//     product_id: i32,
//     consume_pin: u8,
//     add_pin: u8,
//     device: &'a str,
// }

fn main() {
    // let panels = &[
    //     StockPanel {
    //         // Toilet Rolls
    //         product_id: 62,
    //         consume_pin: 9,
    //         add_pin: 10,
    //         device: "/dev/i2c-0",
    //     },
    //     StockPanel {
    //         // Onions
    //         product_id: 43,
    //         consume_pin: 17,
    //         add_pin: 27,
    //         device: "/dev/i2c-1",
    //     },
    // ];

    let client = http_client().unwrap();
    let products = get_products(&client).unwrap();
    let min_product_id = products.iter().map(|p| p.id).min().unwrap_or(1);
    let max_product_id = products.iter().map(|p| p.id).max().unwrap_or(1);

    let mut handles = vec![];
    let (err_tx, err_rx) = mpsc::channel();

    let current_product = Arc::new(Mutex::new(1));

    let (paginator_tx, paginator_rx) = mpsc::channel();
    let (stock_tx, stock_rx) = mpsc::channel();

    // Product paginator buttons
    handles.push(std::thread::spawn({
        let err_tx = err_tx.clone();
        let current_product = current_product.clone();
        let stock_tx = stock_tx.clone();

        move || {
            if let Err(e) = paginator_buttons(
                17,
                27,
                current_product,
                min_product_id,
                max_product_id,
                paginator_tx,
                stock_tx,
            ) {
                err_tx.send(e)
            } else {
                Ok(())
            }
        }
    }));

    // Product paginator screen
    handles.push(std::thread::spawn({
        let err_tx = err_tx.clone();
        move || {
            if let Err(e) = screen("/dev/i2c-1", paginator_rx) {
                err_tx.send(e)
            } else {
                Ok(())
            }
        }
    }));

    // Product stock buttons
    handles.push(std::thread::spawn({
        let err_tx = err_tx.clone();
        let current_product = current_product.clone();

        move || {
            if let Err(e) = stock_buttons(9, 10, current_product, stock_tx) {
                err_tx.send(e)
            } else {
                Ok(())
            }
        }
    }));

    // Product stock screen
    handles.push(std::thread::spawn({
        let err_tx = err_tx.clone();
        move || {
            if let Err(e) = screen("/dev/i2c-0", stock_rx) {
                err_tx.send(e)
            } else {
                Ok(())
            }
        }
    }));

    if let Some(e) = err_rx.iter().next() {
        panic!("{:?}", e);
    }

    for handle in handles {
        handle.join().unwrap().unwrap();
    }
}

fn paginator_buttons(
    left_pin: u8,
    right_pin: u8,
    current_product: Arc<Mutex<i32>>,
    min_product_id: i32,
    max_product_id: i32,
    paginator_tx: mpsc::Sender<ScreenContent>,
    stock_tx: mpsc::Sender<ScreenContent>,
) -> anyhow::Result<()> {
    let gpio = Gpio::new()?;

    // Create a channel to receive interrupt events
    let (tx, rx) = mpsc::channel();

    // Stop InputPin from being dropped
    let mut pins = Vec::new();

    for pin_no in &[left_pin, right_pin] {
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

                let mut current_product_id = current_product.lock().unwrap();

                if pin == left_pin {
                    // consume_product(&client, product_id)?;
                    *current_product_id = (*current_product_id - 1).max(min_product_id)
                } else if pin == right_pin {
                    // add_product(&client, product_id)?;
                    *current_product_id = (*current_product_id + 1).min(max_product_id)
                } else {
                    println!("unknown pin {pin}");
                }

                println!("new product id: {}", *current_product_id);

                paginator_tx.send(ScreenContent {
                    line1: format!("Product {}/{}", *current_product_id, max_product_id),
                    line2: ".".into(),
                    line3: ".".into(),
                })?;

                let product = get_stock_product(&client, *current_product_id)?;
                let lines = multi_line_truncate(&product.product.name, 20, 2);
                let line1 = lines.get(0).cloned().unwrap_or_default();
                let line2 = lines.get(1).cloned().unwrap_or_default();

                stock_tx.send(ScreenContent {
                    line1,
                    line2,
                    line3: format!("{}", product.stock_amount),
                })?;
            }
            _ => {
                // ignore
            }
        }
    }

    println!("Exiting...");
    Ok(())
}

fn stock_buttons(
    consume_pin: u8,
    add_pin: u8,
    current_product: Arc<Mutex<i32>>,
    stock_tx: mpsc::Sender<ScreenContent>,
) -> anyhow::Result<()> {
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
                    consume_product(&client, *current_product.lock().unwrap())?;
                } else if pin == add_pin {
                    add_product(&client, *current_product.lock().unwrap())?;
                } else {
                    println!("unknown pin {pin}");
                }

                let product = get_stock_product(&client, *current_product.lock().unwrap())?;

                let lines = multi_line_truncate(&product.product.name, 20, 2);
                let line1 = lines.get(0).cloned().unwrap_or_default();
                let line2 = lines.get(1).cloned().unwrap_or_default();

                stock_tx.send(ScreenContent {
                    line1,
                    line2,
                    line3: format!("{}", product.stock_amount),
                })?;
            }
            _ => {
                // ignore
            }
        }
    }

    println!("Exiting...");
    Ok(())
}

struct ScreenContent {
    line1: String,
    line2: String,
    line3: String,
}

fn screen(path: &str, rx: mpsc::Receiver<ScreenContent>) -> anyhow::Result<()> {
    let i2c = I2cdev::new(path)?;

    let interface = I2CDisplayInterface::new(i2c);
    let mut disp = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();

    disp.init()
        .map_err(|e| anyhow!("unable to flush: {:?}", e))?;

    let text_style = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(BinaryColor::On)
        .build();

    // let client = http_client()?;

    let mut iter = rx.iter();

    while let Some(message) = iter.next() {
        disp.clear();

        // let product = get_product(&client, product_id)?;

        Text::with_baseline(&message.line1, Point::new(2, 1), text_style, Baseline::Top)
            .draw(&mut disp)
            .map_err(|e| anyhow!("unable to flush: {:?}", e))?;

        Text::with_baseline(&message.line2, Point::new(2, 13), text_style, Baseline::Top)
            .draw(&mut disp)
            .map_err(|e| anyhow!("unable to flush: {:?}", e))?;

        Text::with_baseline(&message.line3, Point::new(2, 25), text_style, Baseline::Top)
            .draw(&mut disp)
            .map_err(|e| anyhow!("unable to flush: {:?}", e))?;

        disp.flush()
            .map_err(|e| anyhow!("unable to flush: {:?}", e))?;
    }

    Ok(())
}

pub fn http_client() -> anyhow::Result<reqwest::blocking::Client> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "GROCY-API-KEY",
        /* good luck! */
        HeaderValue::from_str("G9owQF7bLWDKctBYB6WTZ0xrndSMTLumHJ3k8WtitNozAL9C81")?,
    );
    headers.insert("accept", HeaderValue::from_str("application/json")?);

    Ok(reqwest::blocking::Client::builder()
        .default_headers(headers)
        .build()?)
}

pub fn get_products(client: &reqwest::blocking::Client) -> anyhow::Result<Vec<ProductDetails>> {
    let req = reqwest::blocking::Request::new(
        Method::GET,
        "http://100.117.133.36:9283/api/objects/products".parse()?,
    );
    let res: Vec<ProductDetails> = client.execute(req)?.json()?;
    Ok(res)
}

pub fn get_stock_product(
    client: &reqwest::blocking::Client,
    id: i32,
) -> anyhow::Result<StockProduct> {
    let req = reqwest::blocking::Request::new(
        Method::GET,
        format!("http://100.117.133.36:9283/api/stock/products/{id}").parse()?,
    );
    let res: StockProduct = client.execute(req)?.json()?;
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
pub struct StockProduct {
    #[serde(default = "default_stock_amount")]
    pub stock_amount: f32,
    #[serde(default = "default_product")]
    pub product: ProductDetails,
}

#[derive(Deserialize, Debug, Default)]
pub struct ProductDetails {
    pub id: i32,
    pub name: String,
}

fn default_stock_amount() -> f32 {
    0.0
}

fn default_product() -> ProductDetails {
    Default::default()
}

fn multi_line_truncate(input: &str, max_length: u8, max_lines: u32) -> Vec<String> {
    let mut buf = String::new();
    let mut i = 0;
    let mut l = 0;

    for char in input.chars() {
        if i == max_length {
            i = 0;
            l += 1;

            if l == max_lines {
                break;
            }
            buf += "\n";
        }

        buf += &char.to_string();
        i += 1;
    }

    buf.split("\n").map(ToString::to_string).collect()
}
