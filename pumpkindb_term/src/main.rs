// Copyright (c) 2017, All Contributors (see CONTRIBUTORS file)
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.


extern crate config;
extern crate rustyline;
extern crate ansi_term;
extern crate uuid;
#[macro_use]
extern crate clap;
extern crate byteorder;

extern crate pumpkinscript;
extern crate pumpkindb_engine;

use std::io::prelude::*;
use std::net::TcpStream;
use std::fmt::Write;
use std::io::Write as IoWrite;
use std::str;

use byteorder::{ByteOrder, BigEndian};

use rustyline::error::ReadlineError;
use rustyline::Editor;
use rustyline::history::History;

use ansi_term::Colour::{Red, Cyan};

use uuid::Uuid;

use clap::{Arg, App};

use pumpkindb_engine::script;
use pumpkinscript::compose::*;
use pumpkinscript::compose::Item::*;

fn print_item(s: &mut String, data: &[u8]) {
    if data.iter()
        .all(|c| *c >= 0x20 && *c <= 0x7e) {
        let _ = write!(s,
                       "{:?} ",
                       str::from_utf8(data)
                           .unwrap());
    } else {
        let _ = write!(s, "0x");
        for b in Vec::from(data) {
            let _ = write!(s, "{:02x}", b);
        }
        let _ = write!(s, " ");
    }
}

fn main() {

    let args = App::new("PumpkinDB Terminal")
        .version(crate_version!())
        .about("Command-line access to PumpkinDB")
        .setting(clap::AppSettings::ColoredHelp)
        .arg(Arg::with_name("address")
            .help("Address to connect to")
            .required(true)
            .default_value("127.0.0.1:9981")
            .index(1))
        .get_matches();

    let _ = config::merge(config::Environment::new("pumpkindb"));
    let _ = config::set_default("prompt", "PumpkinDB> ");
    let formatted_prompt = format!("{}", config::get_str("prompt").unwrap());
    let mut current_prompt = formatted_prompt.as_str();

    let address = args.value_of("address").unwrap();
    let mut stream = match TcpStream::connect(address) {
        Ok(s) => s,
        Err(err) => {
            println!("Can't connect to {}, error: {}", address, err);
            return;
        }
    };

    let mut rl = Editor::<()>::new();
    let mut r = Vec::new();

    let mut multine = History::new();
    println!("Connected to PumpkinDB at {}", address);
    println!("To send an expression, end it with `.`");
    println!("Type \\h for help.");
    loop {
        match rl.readline(current_prompt) {
            Ok(text) => {
                let mut program = String::new();
                let text_str = text.as_str();
                let text_bytes = text_str.as_bytes();
                if text_bytes.len() >= 2 && text_bytes[0] == b'\\' {
                    if text_bytes[1] == b'h' {
                        println!("\nTo send an expression, end it with `.`");
                        println!("To trace a value in the script use TRACE instruction");
                        println!("To quit, hit ^D");
                        println!("Further help online at http://pumpkindb.org/doc/");
                        println!("Missing a feature? Let us know at \
                                  https://github.com/PumpkinDB/PumpkinDB/issues/\n");
                    }
                } else if text_str.len() > 0 && text_bytes[text_str.len() - 1] == 46u8 {
                    let rest = str::from_utf8(&text.as_bytes()[..text_str.len() - 1]).unwrap();
                    multine.add(&rest);
                    for i in 0..multine.len() {
                        program.push_str(multine.get(i).unwrap().as_str());
                    }
                    multine.clear();
                    current_prompt = formatted_prompt.as_str();
                } else {
                    multine.add(&text);
                    multine.add(&" ");
                    current_prompt = "..> ";
                }
                if program.len() > 0 {
                    rl.add_history_entry(format!("{}.", &program).as_str());
                    match pumpkinscript::parse(&program) {
                        Ok(compiled) => {
                            let uuid = Uuid::new_v4();
                            let trace: Vec<u8> = Program(vec![
                                Data(&[1]), Instruction("WRAP"),
                                Data("TRACE".as_bytes()),
                                Instruction("SWAP"),
                                Instruction("CONCAT"),
                                Data(uuid.as_bytes()),
                                Instruction("PUBLISH"),
                               ]).into();
                            let msg: Vec<u8> = Program(vec![
                                                            Data(uuid.as_bytes()),
                                                            Instruction("SUBSCRIBE"),
                                                            InstructionRef("___subscription___"),
                                                            Instruction("SET"),
                                                            Data(&trace),
                                                            InstructionRef("TRACE"),
                                                            Instruction("DEF"),
                                                            Data(&compiled),
                                                            Instruction("TRY"),
                                                            Instruction("STACK"),
                                                            Data("RESULT".as_bytes()),
                                                            Instruction("SWAP"),
                                                            Instruction("CONCAT"),
                                                            Data(uuid.as_bytes()),
                                                            Instruction("PUBLISH"),
                                                            Instruction("___subscription___"),
                                                            Instruction("UNSUBSCRIBE")
                               ]).into();
                            let mut buf = [0u8; 4];

                            BigEndian::write_u32(&mut buf, msg.len() as u32);
                            stream.write_all(buf.as_ref()).unwrap();
                            stream.write_all(msg.as_ref()).unwrap();

                            let mut done = false;

                            while !done {
                                stream.read(&mut buf).unwrap();
                                let msg_len = BigEndian::read_u32(&mut buf);

                                let s_ref = <TcpStream as Read>::by_ref(&mut stream);

                                r.clear();

                                match s_ref.take(msg_len as u64).read_to_end(&mut r) {
                                    Ok(0) => {}
                                    Ok(_) => {
                                        if r[0..5].to_vec() == b"TRACE" {
                                            let input = r[5..msg_len as usize].to_vec();
                                            let mut s = String::new();
                                            if cfg!(target_os = "windows") {
                                                let _ = write!(&mut s, "Trace: ");
                                            } else {
                                                let _ = write!(&mut s,
                                                               "{}", Cyan.paint("Trace: "));
                                            }
                                            match pumpkinscript::binparser::data(&input.clone()) {
                                                pumpkinscript::ParseResult::Done(_, data) => {
                                                    let (_, size) = pumpkinscript::binparser::data_size(data)
                                                        .unwrap();
                                                    let data = &data[script::offset_by_size(size)..];
                                                    print_item(&mut s, data);
                                                },
                                                e => {
                                                    panic!("{:?}", e);
                                                }
                                            }
                                            println!("{}", s);
                                        } else if r[0..6].to_vec() == b"RESULT" {
                                            let mut input = r[6..msg_len as usize].to_vec();
                                            done = true;
                                            let mut top_level = true;
                                            let mut s = String::new();
                                            while input.len() > 0 {
                                                match pumpkinscript::binparser::data(&input.clone()) {
                                                    pumpkinscript::ParseResult::Done(rest, data) => {
                                                        let (_, size) = pumpkinscript::binparser::data_size(data)
                                                            .unwrap();
                                                        let data = &data[script::offset_by_size(size)..];

                                                        input = Vec::from(rest);

                                                        if rest.len() == 0 && top_level {
                                                            top_level = false;
                                                            if data.len() > 0 {
                                                                if cfg!(target_os = "windows") {
                                                                    let _ = write!(&mut s, "Error: ");
                                                                } else {
                                                                    let _ = write!(&mut s,
                                                                                   "{}",
                                                                                   Red.paint("Error: "));
                                                                }
                                                                input = Vec::from(data);
                                                            }
                                                        } else {
                                                            print_item(&mut s, data);
                                                        }
                                                    }
                                                    e => {
                                                        panic!("{:?}", e);
                                                    }
                                                }
                                            }
                                            println!("{}", s);
                                        }
                                    }
                                    Err(e) => {
                                        panic!("{}", e);
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            println!("Script error: {:?}", err);
                        }
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("Aborted");
                break;
            }
            Err(ReadlineError::Eof) => {
                println!("Exiting");
                break;
            }
            Err(err) => {
                println!("Error: {:?}", err);
                break;
            }
        }


    }
}
