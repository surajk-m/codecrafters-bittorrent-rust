use anyhow::Context;
use bittorrent_starter_rust::peer::*;
use bittorrent_starter_rust::torrent::{self, Torrent};
use bittorrent_starter_rust::tracker::*;
use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use serde_bencode;
use serde_json::{Map, Value};
use sha1::{Digest, Sha1};
use std::fs::read;
use std::net::SocketAddrV4;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const DEFAULT_PORT: u16 = 6881;
const DEFAULT_COMPACT: u8 = 1;
const BLOCK_MAX: usize = 1 << 14;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Decode {
        value: String,
    },
    Info {
        torrent: PathBuf,
    },
    Peers {
        torrent: PathBuf,
    },
    Handshake {
        torrent: PathBuf,
        peer: String,
    },
    #[clap(name = "download_piece")]
    DownloadPiece {
        #[arg(short)]
        output: PathBuf,
        torrent: PathBuf,
        piece: usize,
    },
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
        Command::Handshake { torrent, peer } => {
            let dot_torrent = std::fs::read(torrent).context("read torrent file")?;
            let t: Torrent =
                serde_bencode::from_bytes(&dot_torrent).context("parse torrent file")?;

            let info_hash = t.info_hash()?;
            let peer = peer.parse::<SocketAddrV4>().context("parse peer address")?;
            let mut peer = tokio::net::TcpStream::connect(peer)
                .await
                .context("connect to peer")?;
            let mut handshake = Handshake::new(info_hash, *b"00112233445566778899");
            {
                let handshake_bytes =
                    &mut handshake as *mut Handshake as *mut [u8; std::mem::size_of::<Handshake>()];
                // Safety: Handshake is a POD with repr(c)
                let handshake_bytes: &mut [u8; std::mem::size_of::<Handshake>()] =
                    unsafe { &mut *handshake_bytes };
                peer.write_all(handshake_bytes)
                    .await
                    .context("write handshake")?;
                peer.read_exact(handshake_bytes)
                    .await
                    .context("read handshake")?;
            }
            assert_eq!(handshake.length, 19);
            assert_eq!(&handshake.bittorrent, b"BitTorrent protocol");
            println!("Peer ID: {}", hex::encode(&handshake.peer_id));
        }
        Command::DownloadPiece {
            output,
            torrent,
            piece: piece_i,
        } => {
            let dot_torrent = std::fs::read(torrent).context("read torrent file")?;
            let t: Torrent =
                serde_bencode::from_bytes(&dot_torrent).context("parse torrent file")?;
            let length = if let torrent::Keys::SingleFile { length } = t.info.keys {
                length
            } else {
                todo!();
            };
            assert!(piece_i < t.info.pieces.0.len());

            let info_hash = t.info_hash()?;
            let request = TrackerRequest {
                peer_id: String::from("00112233445566778899"),
                port: 6881,
                uploaded: 0,
                downloaded: 0,
                left: length,
                compact: 1,
            };

            let url_params =
                serde_urlencoded::to_string(&request).context("url-encode tracker parameters")?;
            let tracker_url = format!(
                "{}?{}&info_hash={}",
                t.announce,
                url_params,
                &urlencode(&info_hash)
            );
            let response = reqwest::get(tracker_url).await.context("query tracker")?;
            let response = response.bytes().await.context("fetch tracker response")?;
            let tracker_info: TrackerResponse =
                serde_bencode::from_bytes(&response).context("parse tracker response")?;

            let peer = &tracker_info.peers.0[0];
            let mut peer = tokio::net::TcpStream::connect(peer)
                .await
                .context("connect to peer")?;
            let mut handshake = Handshake::new(info_hash, *b"00112233445566778899");
            {
                let handshake_bytes = handshake.as_bytes_mut();
                peer.write_all(handshake_bytes)
                    .await
                    .context("write handshake")?;
                peer.read_exact(handshake_bytes)
                    .await
                    .context("read handshake")?;
            }
            assert_eq!(handshake.length, 19);
            assert_eq!(&handshake.bittorrent, b"BitTorrent protocol");
            // println!("Peer ID: {}", hex::encode(&handshake.peer_id));

            let mut peer = tokio_util::codec::Framed::new(peer, MessageFramer);
            let bitfield = peer
                .next()
                .await
                .expect("peer always sends a bitfields")
                .context("peer message was invalid")?;
            assert_eq!(bitfield.tag, MessageTag::Bitfield);
            // NOTE: we assume that the bitfield covers all pieces

            peer.send(Message {
                tag: MessageTag::Interested,
                payload: Vec::new(),
            })
            .await
            .context("send interested message")?;

            let unchoke = peer
                .next()
                .await
                .expect("peer always sends an unchoke")
                .context("peer message was invalid")?;
            assert_eq!(unchoke.tag, MessageTag::Unchoke);
            assert!(unchoke.payload.is_empty());

            let piece_hash = &t.info.pieces.0[piece_i];
            let piece_size = if piece_i == t.info.pieces.0.len() - 1 {
                let md = length % t.info.plength;
                if md == 0 {
                    t.info.plength
                } else {
                    md
                }
            } else {
                t.info.plength
            };
            // the + (BLOCK_MAX - 1) rounds up
            let nblocks = (piece_size + (BLOCK_MAX - 1)) / BLOCK_MAX;
            // eprintln!("{nblocks} blocks of at most {BLOCK_MAX} to reach {piece_size}");
            let mut all_blocks = Vec::with_capacity(piece_size);
            for block in 0..nblocks {
                let block_size = if block == nblocks - 1 {
                    let md = piece_size % BLOCK_MAX;
                    if md == 0 {
                        BLOCK_MAX
                    } else {
                        md
                    }
                } else {
                    BLOCK_MAX
                };
                // eprintln!("block #{block} is {block_size}b");
                let mut request = Request::new(
                    piece_i as u32,
                    (block * BLOCK_MAX) as u32,
                    block_size as u32,
                );
                let request_bytes = Vec::from(request.as_bytes_mut());
                peer.send(Message {
                    tag: MessageTag::Request,
                    payload: request_bytes,
                })
                .await
                .with_context(|| format!("send request for block {block}"))?;

                let piece = peer
                    .next()
                    .await
                    .expect("peer always sends a piece")
                    .context("peer message was invalid")?;
                assert_eq!(piece.tag, MessageTag::Piece);
                assert!(!piece.payload.is_empty());

                let piece = Piece::ref_from_bytes(&piece.payload[..])
                    .expect("always get all Piece response fields from peer");
                assert_eq!(piece.index() as usize, piece_i);
                assert_eq!(piece.begin() as usize, block * BLOCK_MAX);
                assert_eq!(piece.block().len(), block_size);
                all_blocks.extend(piece.block());
            }

            assert_eq!(all_blocks.len(), piece_size);

            let mut hasher = Sha1::new();
            hasher.update(&all_blocks);
            let hash: [u8; 20] = hasher
                .finalize()
                .try_into()
                .expect("GenericArray<_, 20> == [_; 20]");
            assert_eq!(&hash, piece_hash);

            tokio::fs::write(&output, all_blocks)
                .await
                .context("write out downloaded piece")?;
            println!("Piece {piece_i} downloaded to {}.", output.display());
        }
    }
    Ok(())
}
