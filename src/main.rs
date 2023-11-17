use anyhow::Context;
use bittorrent_starter_rust::torrent::{self, Torrent};
use bittorrent_starter_rust::tracker::*;
use clap::{Parser, Subcommand};
use serde_bencode;
use serde_json::{Map, Value};
use std::fs::read;
use std::path::PathBuf;

const DEFAULT_PORT: u16 = 6881;
const DEFAULT_COMPACT: u8 = 1;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Decode { value: String },
    Info { torrent: PathBuf },
    Peers { torrent: PathBuf },
}

fn decode_bencoded_value(encoded_value: &str) -> Result<(Value, &str), anyhow::Error> {
    match encoded_value.chars().next() {
        Some('i') => {
            let (n, rest) = encoded_value
                .split_at(1)
                .1
                .split_once('e')
                .and_then(|(digits, rest)| {
                    let n = digits.parse::<i64>().ok()?;
                    Some((n.into(), rest))
                })
                .ok_or_else(|| anyhow::anyhow!("Failed to parse integer"))?;
            Ok((n, rest))
        }
        Some('l') => {
            let mut values = Vec::new();
            let mut rest = encoded_value.split_at(1).1;
            while !rest.is_empty() && rest.starts_with(|c| c != 'e') {
                let (v, remainder) = decode_bencoded_value(rest)?;
                values.push(v);
                rest = remainder;
            }
            Ok((Value::Array(values), &rest[1..]))
        }
        Some('d') => {
            let mut dict = Map::new();
            let mut rest = encoded_value.split_at(1).1;
            while !rest.is_empty() && rest.starts_with(|c| c != 'e') {
                let (key, remainder) = decode_bencoded_value(rest)?;
                let (value, remainder) = decode_bencoded_value(remainder)?;
                if let Value::String(key_str) = key {
                    dict.insert(key_str, value);
                } else {
                    return Err(anyhow::anyhow!("Dictionary keys must be strings"));
                }
                rest = remainder;
            }
            let json_map: Map<String, Value> = dict.into_iter().collect();
            Ok((Value::Object(json_map), &rest[1..]))
        }
        Some('0'..='9') => {
            let (len, rest) = encoded_value.split_once(':').ok_or_else(|| {
                anyhow::anyhow!("Failed to split length and remainder for string")
            })?;
            let len = len
                .parse::<usize>()
                .map_err(|_| anyhow::anyhow!("Failed to parse length"))?;
            Ok((Value::String(rest[..len].to_string()), &rest[len..]))
        }
        _ => Err(anyhow::anyhow!(
            "Unhandled encoded value: {}",
            encoded_value
        )),
    }
}

fn urlencode(t: &[u8; 20]) -> String {
    let mut encoded = String::with_capacity(3 * t.len());
    for &byte in t {
        encoded.push('%');
        encoded.push_str(&hex::encode(&[byte]));
    }
    encoded
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    match args.command {
        Command::Decode { value } => {
            eprintln!("Logs from your program will appear here!");
            let decoded_value = decode_bencoded_value(&value)?;
            println!("{}", decoded_value.0.to_string());
        }
        Command::Info { torrent } => {
            let dot_torrent = read(&torrent).context("read torrent file")?;
            let t: Torrent =
                serde_bencode::from_bytes(&dot_torrent).context("parse torrent file")?;
            eprintln!("{t:?}");
            println!("Tracker URL: {}", t.announce);

            match &t.info.keys {
                torrent::Keys::SingleFile { length } => println!("Length: {}", length),
                _ => todo!(),
            }

            let info_hash = t.info_hash()?;
            println!("Info Hash: {}", hex::encode(&info_hash));
            println!("Piece Length: {}", t.info.plength);
            println!("Piece Hashes:");
            for hash in t.info.pieces.0 {
                println!("{}", hex::encode(&hash));
            }
        }

        Command::Peers { torrent } => {
            let dot_torrent = std::fs::read(torrent).context("read torrent file")?;
            let t: Torrent =
                serde_bencode::from_bytes(&dot_torrent).context("parse torrent file")?;
            let length = match t.info.keys {
                torrent::Keys::SingleFile { length } => length,
                _ => {
                    todo!();
                }
            };

            let info_hash = t.info_hash()?;
            let request = TrackerRequest {
                peer_id: String::from("00112233445566778899"),
                port: DEFAULT_PORT,
                uploaded: 0,
                downloaded: 0,
                left: length,
                compact: DEFAULT_COMPACT,
            };

            let url_params =
                serde_urlencoded::to_string(&request).context("url-encode tracker parameters")?;
            let tracker_url = format!(
                "{}?{}&info_hash={}",
                t.announce,
                url_params,
                &urlencode(&info_hash)
            );
            let response = reqwest::get(&tracker_url).await.context("query tracker")?;
            let response = response.bytes().await.context("fetch tracker response")?;
            let response: TrackerResponse =
                serde_bencode::from_bytes(&response).context("parse tracker response")?;
            for peer in &response.peers.0 {
                println!("{}:{}", peer.ip(), peer.port());
            }
        }
    }
    Ok(())
}
