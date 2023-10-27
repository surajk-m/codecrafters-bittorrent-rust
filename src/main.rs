use serde_json;
use std::env;

fn decode_bencoded_value(encoded_value: &str) -> serde_json::Value {
    if encoded_value.starts_with('i') {
        // Integer case: i<number>e
        let end_index = encoded_value.find('e').unwrap();
        let number_string = &encoded_value[1..end_index];
        let number = number_string.parse::<i64>().unwrap();
        return serde_json::Value::Number(serde_json::Number::from(number));
    } else if encoded_value.chars().next().unwrap().is_digit(10) {
        // String case: <length>:<string>
        let colon_index = encoded_value.find(':').unwrap();
        let number_string = &encoded_value[..colon_index];
        let number = number_string.parse::<usize>().unwrap();
        let string = &encoded_value[colon_index + 1..colon_index + 1 + number];
        return serde_json::Value::String(string.to_string());
    } else {
        panic!("Unhandled encoded value: {}", encoded_value)
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let command = &args[1];

    if command == "decode" {
        eprintln!("Logs from your program will appear here!");

        let encoded_value = &args[2];
        let decoded_value = decode_bencoded_value(encoded_value);
        println!("{}", decoded_value.to_string());
    } else {
        eprintln!("unknown command: {}", args[1])
    }
}
