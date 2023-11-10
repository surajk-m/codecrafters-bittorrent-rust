use anyhow::Context;
use clap::{Parser, Subcommand};
use hashes::Hashes;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha1::{Digest, Sha1};
use std::fs::read;
use std::path::PathBuf;

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
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct Torrent {
    announce: String,
    info: Info,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct Info {
    name: String,
    #[serde(rename = "piece length")]
    plength: usize,
    pieces: Hashes,
    #[serde(flatten)]
    keys: Keys,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
enum Keys {
    SingleFile { length: usize },
    MultiFile { files: Vec<File> },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct File {
    length: usize,
    path: Vec<String>,
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

fn main() -> anyhow::Result<()> {
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

            match t.info.keys {
                Keys::SingleFile { length } => println!("Length: {}", length),
                _ => todo!(),
            }

            let info_encoded =
                serde_bencode::to_bytes(&t.info).context("re-encode info section")?;
            let mut hasher = Sha1::new();
            hasher.update(&info_encoded);
            let info_hash = hasher.finalize();
            println!("Info Hash: {}", hex::encode(&info_hash));
            println!("Piece Length: {}", t.info.plength);
            println!("Piece Hashes:");
            for hash in t.info.pieces.0 {
                println!("{}", hex::encode(&hash));
            }
        }
    }

    Ok(())
}

mod hashes {
    use serde::de::{self, Deserialize, Deserializer, Visitor};
    use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
    use std::fmt;

    #[derive(Debug, Clone)]
    pub struct Hashes(pub Vec<[u8; 20]>);
    struct HashesVisitor;

    impl<'de> Visitor<'de> for HashesVisitor {
        type Value = Hashes;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a byte string whose length is a multiple of 20")
        }

        fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            if v.len() % 20 != 0 {
                return Err(E::custom(format!("length is {}", v.len())));
            }
            // TODO: use array_chunks when stable
            Ok(Hashes(
                v.chunks_exact(20)
                    .map(|slice_20| slice_20.try_into().expect("guaranteed to be length 20"))
                    .collect(),
            ))
        }
    }

    impl<'de> Deserialize<'de> for Hashes {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_bytes(HashesVisitor)
        }
    }

    impl Serialize for Hashes {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let single_slice = self.0.concat();
            serializer.serialize_bytes(&single_slice)
        }
    }
}
