//! Avatar resolution + local caching (TD-033).
//!
//! The ENS `avatar` text record is not always something a browser `<img>`
//! can load: it may be an `http(s)://` URL, an `ipfs://` URI, a `data:`
//! URI, or an `eip155:<chain>/erc721:<contract>/<tokenId>` NFT reference.
//! This module resolves any of those down to concrete image bytes and
//! writes them to a shared volume so the API can serve every avatar from
//! its own origin. Only the `eip155:` branch touches L1; the rest is a
//! cheap string transform plus an HTTP fetch.
//!
//! Resolution is best-effort: any failure returns `Ok(None)` (with a
//! `warn!`) rather than erroring, so a hostile or unreachable avatar
//! record never blocks the ENS sweep. Callers must treat `Ok(None)` as
//! "leave the previously cached file in place" — see
//! `runner::resolve_and_upsert_orchestrator`.

use alloy::{primitives::U256, sol, sol_types::SolCall};
use anyhow::{anyhow, Context, Result};
use base64::Engine;
use livepeer_core::rpc::{BlockTag, Provider};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, warn};

/// Public IPFS gateway used to rewrite `ipfs://` / `ipns://` URIs into
/// browser/HTTP-loadable URLs at fetch time.
const IPFS_GATEWAY: &str = "https://ipfs.io/ipfs/";
const IPNS_GATEWAY: &str = "https://ipfs.io/ipns/";
/// Gateway hosts (scheme + authority, no trailing slash) tried in order for
/// any IPFS/IPNS path. Public gateways are individually flaky/slow, so we
/// fail over across several rather than giving up on the first timeout.
const IPFS_GATEWAY_HOSTS: &[&str] = &[
    "https://ipfs.io",
    "https://dweb.link",
    "https://gateway.pinata.cloud",
    "https://nftstorage.link",
    "https://4everland.io",
];
/// Hard cap on a single fetched payload (image or NFT metadata). ENS
/// avatars are small; this just bounds a hostile record's blast radius.
const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Per-request timeout. Public IPFS gateways are slow to serve cold
/// content, so this is generous; the gateway fail-over bounds total time.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Extra attempts per candidate URL before moving on (transient errors).
const FETCH_RETRIES: usize = 1;
/// Extensions we know how to sniff and serve. Used both to pick the
/// stored filename and to clean up stale files when an avatar changes type.
const KNOWN_EXTS: &[&str] = &["png", "jpg", "gif", "webp", "svg"];

sol! {
    interface Erc721 {
        function tokenURI(uint256 tokenId) external view returns (string memory);
    }
    interface Erc1155 {
        function uri(uint256 id) external view returns (string memory);
    }
}

/// Configured avatar store directory (env `AVATAR_STORE_DIR`), resolved
/// once. `None` disables caching entirely — the enricher then behaves as
/// it did before TD-033 (raw text record only). Mirrors the API's reader
/// in `livepeer-api`.
pub fn store_dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| std::env::var_os("AVATAR_STORE_DIR").map(PathBuf::from))
        .as_deref()
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            // Browser-ish UA: some CDNs reject obvious bot agents for image
            // hotlinks. Honest token appended for operators.
            .user_agent("Mozilla/5.0 (compatible; livepeer-enricher/1.0; +https://livepeer.org)")
            .build()
            .expect("building avatar http client")
    })
}

/// Resolve `raw_avatar` to image bytes and persist them at
/// `<dir>/<address>.<ext>`, returning the stored extension. Best-effort:
/// returns `Ok(None)` on any resolution/fetch/validation failure.
pub async fn resolve_and_store(
    l1: &Provider,
    dir: &Path,
    address: &str,
    raw_avatar: &str,
) -> Result<Option<String>> {
    let bytes = match resolve_bytes(l1, raw_avatar).await {
        Ok(Some(b)) => b,
        Ok(None) => return Ok(None),
        Err(e) => {
            // `{:#}` prints the full anyhow context chain (e.g. the
            // underlying timeout), not just the top "fetching <url>" frame.
            warn!(address = %address, avatar = %raw_avatar, error = format!("{e:#}"), "avatar resolution failed; keeping any existing cached file");
            return Ok(None);
        }
    };
    let Some(ext) = sniff_ext(&bytes) else {
        warn!(address = %address, avatar = %raw_avatar, "resolved avatar is not a recognized image; skipping");
        return Ok(None);
    };
    write_atomically(dir, address, ext, &bytes)
        .await
        .with_context(|| format!("writing avatar for {address}"))?;
    debug!(address = %address, ext, bytes = bytes.len(), "cached avatar");
    Ok(Some(ext.to_string()))
}

/// Remove any cached avatar files for `address` (all known extensions).
/// Used when an orchestrator's ENS avatar record disappears.
pub async fn clear(dir: &Path, address: &str) -> Result<()> {
    for ext in KNOWN_EXTS {
        let path = dir.join(format!("{address}.{ext}"));
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
        }
    }
    Ok(())
}

/// Resolve any supported avatar reference to raw image bytes. `Ok(None)`
/// means "recognized but empty/unsupported in a non-error way".
async fn resolve_bytes(l1: &Provider, raw: &str) -> Result<Option<Vec<u8>>> {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix("eip155:") {
        return resolve_nft(l1, rest).await.map(Some);
    }
    // http(s), ipfs://, ipns://, data: — direct fetch/decode.
    fetch_uri_bytes(raw).await.map(Some)
}

/// Resolve an `eip155:<chain>/erc721|erc1155:<contract>/<tokenId>` NFT
/// reference: read the on-chain token URI, fetch its metadata JSON, then
/// fetch the `image` it points at.
async fn resolve_nft(l1: &Provider, rest: &str) -> Result<Vec<u8>> {
    // rest looks like: "1/erc721:0xCONTRACT/TOKENID"
    let mut parts = rest.splitn(3, '/');
    let _chain = parts.next().ok_or_else(|| anyhow!("nft: missing chain"))?;
    let asset = parts.next().ok_or_else(|| anyhow!("nft: missing asset"))?;
    let token_id_str = parts
        .next()
        .ok_or_else(|| anyhow!("nft: missing token id"))?;
    let (standard, contract) = asset
        .split_once(':')
        .ok_or_else(|| anyhow!("nft: malformed asset reference"))?;
    let token_id = U256::from_str_radix(token_id_str, 10)
        .with_context(|| format!("nft: bad token id {token_id_str}"))?;

    let token_uri = match standard {
        "erc721" => {
            let data = format!(
                "0x{}",
                alloy::hex::encode(Erc721::tokenURICall { tokenId: token_id }.abi_encode())
            );
            let raw = eth_call_bytes(l1, contract, &data).await?;
            Erc721::tokenURICall::abi_decode_returns(&raw, true)?._0
        }
        "erc1155" => {
            let data = format!(
                "0x{}",
                alloy::hex::encode(Erc1155::uriCall { id: token_id }.abi_encode())
            );
            let raw = eth_call_bytes(l1, contract, &data).await?;
            // ERC-1155 allows a `{id}` template, substituted with the
            // lowercase 64-hex-padded token id.
            let hex_id = format!("{token_id:064x}");
            Erc1155::uriCall::abi_decode_returns(&raw, true)?
                ._0
                .replace("{id}", &hex_id)
        }
        other => return Err(anyhow!("nft: unsupported token standard {other}")),
    };

    let metadata = fetch_uri_bytes(token_uri.trim()).await?;
    let json: serde_json::Value =
        serde_json::from_slice(&metadata).context("nft: metadata is not JSON")?;
    let image = json
        .get("image")
        .or_else(|| json.get("image_url"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("nft: metadata has no image field"))?;
    fetch_uri_bytes(image.trim()).await
}

/// Fetch bytes from a non-NFT URI: `data:`, `http(s)://`, `ipfs://`, or
/// `ipns://`. For IPFS/IPNS content, fails over across multiple public
/// gateways; for any URL, retries a transient error before giving up.
async fn fetch_uri_bytes(uri: &str) -> Result<Vec<u8>> {
    if let Some(rest) = uri.strip_prefix("data:") {
        return decode_data_uri(rest);
    }
    let url = to_http_url(uri)?;
    let candidates = fetch_candidates(&url);
    let mut last_err: Option<anyhow::Error> = None;
    for candidate in &candidates {
        for attempt in 0..=FETCH_RETRIES {
            match fetch_once(candidate).await {
                Ok(bytes) => return Ok(bytes),
                Err(e) => {
                    last_err = Some(e);
                    if attempt < FETCH_RETRIES {
                        sleep(Duration::from_millis(400)).await;
                    }
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("no fetch candidates for {url}")))
}

/// Build the ordered list of URLs to try for `url`. IPFS/IPNS gateway URLs
/// expand to the same path across every gateway in `IPFS_GATEWAY_HOSTS`;
/// any other URL is tried as-is.
fn fetch_candidates(url: &str) -> Vec<String> {
    if let Some(suffix) = ipfs_path_suffix(url) {
        IPFS_GATEWAY_HOSTS
            .iter()
            .map(|host| format!("{host}{suffix}"))
            .collect()
    } else {
        vec![url.to_string()]
    }
}

/// If `url` is a path-style IPFS/IPNS gateway URL, return the
/// `/ipfs/<cid>...` or `/ipns/<name>...` suffix so it can be retried on
/// other gateways. Subdomain-style gateways (e.g. `<cid>.ipfs.dweb.link`)
/// have no such suffix and are fetched as-is.
fn ipfs_path_suffix(url: &str) -> Option<&str> {
    ["/ipfs/", "/ipns/"]
        .iter()
        .filter_map(|marker| url.find(marker).map(|idx| &url[idx..]))
        .next()
}

async fn fetch_once(url: &str) -> Result<Vec<u8>> {
    let resp = http_client()
        .get(url)
        .send()
        .await
        .with_context(|| format!("fetching {url}"))?
        .error_for_status()
        .with_context(|| format!("non-success status from {url}"))?;
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_BYTES {
            return Err(anyhow!("payload too large: {len} bytes from {url}"));
        }
    }
    let bytes = resp.bytes().await.context("reading response body")?;
    if bytes.len() > MAX_BYTES {
        return Err(anyhow!(
            "payload too large: {} bytes from {url}",
            bytes.len()
        ));
    }
    Ok(bytes.to_vec())
}

/// Rewrite an ipfs/ipns URI to an HTTP gateway URL; pass http(s) through.
fn to_http_url(uri: &str) -> Result<String> {
    if uri.starts_with("http://") || uri.starts_with("https://") {
        Ok(uri.to_string())
    } else if let Some(cid) = uri.strip_prefix("ipfs://") {
        // Some records redundantly include the `ipfs/` path segment.
        let cid = cid.strip_prefix("ipfs/").unwrap_or(cid);
        Ok(format!("{IPFS_GATEWAY}{cid}"))
    } else if let Some(name) = uri.strip_prefix("ipns://") {
        Ok(format!("{IPNS_GATEWAY}{name}"))
    } else {
        Err(anyhow!("unsupported avatar URI scheme: {uri}"))
    }
}

/// Decode the body of a `data:` URI (the part after `data:`).
fn decode_data_uri(rest: &str) -> Result<Vec<u8>> {
    let (meta, payload) = rest
        .split_once(',')
        .ok_or_else(|| anyhow!("malformed data URI"))?;
    if meta.contains(";base64") {
        base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .context("decoding base64 data URI")
    } else {
        // Percent-decoded text payload (e.g. an inline SVG).
        Ok(percent_decode(payload).into_bytes())
    }
}

/// Minimal percent-decoder for text `data:` URIs (avoids a new dep).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Identify the image type from leading magic bytes. Returns the file
/// extension we store under, or `None` for unrecognized payloads.
fn sniff_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF8") {
        Some("gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if is_svg(bytes) {
        Some("svg")
    } else {
        None
    }
}

/// SVG is text; sniff by looking for an `<svg` tag near the start, after
/// skipping any XML prolog/BOM/whitespace.
fn is_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let text = String::from_utf8_lossy(head);
    let lowered = text.to_ascii_lowercase();
    lowered.contains("<svg")
}

async fn write_atomically(dir: &Path, address: &str, ext: &str, bytes: &[u8]) -> Result<()> {
    tokio::fs::create_dir_all(dir)
        .await
        .with_context(|| format!("creating avatar dir {}", dir.display()))?;
    // Drop any previously-cached file under a different extension so a
    // type change (e.g. png -> svg) doesn't leave a stale image behind.
    for other in KNOWN_EXTS.iter().filter(|e| **e != ext) {
        let stale = dir.join(format!("{address}.{other}"));
        let _ = tokio::fs::remove_file(&stale).await;
    }
    let final_path = dir.join(format!("{address}.{ext}"));
    let tmp_path = dir.join(format!("{address}.{ext}.tmp"));
    tokio::fs::write(&tmp_path, bytes)
        .await
        .with_context(|| format!("writing {}", tmp_path.display()))?;
    tokio::fs::rename(&tmp_path, &final_path)
        .await
        .with_context(|| format!("renaming into {}", final_path.display()))?;
    Ok(())
}

async fn eth_call_bytes(l1: &Provider, to: &str, data: &str) -> Result<Vec<u8>> {
    let value = l1.eth_call(to, data, BlockTag::Latest).await?;
    let s = value
        .as_str()
        .ok_or_else(|| anyhow!("eth_call response was not a hex string"))?;
    alloy::hex::decode(s.trim_start_matches("0x")).context("decoding eth_call return hex")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_common_image_types() {
        assert_eq!(sniff_ext(&[0x89, b'P', b'N', b'G', 0x0D]), Some("png"));
        assert_eq!(sniff_ext(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(sniff_ext(b"GIF89a..."), Some("gif"));
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBP");
        assert_eq!(sniff_ext(&webp), Some("webp"));
        assert_eq!(
            sniff_ext(b"<?xml version=\"1.0\"?><svg xmlns=\"...\">"),
            Some("svg")
        );
        assert_eq!(sniff_ext(b"not an image"), None);
    }

    #[test]
    fn ipfs_urls_fan_out_across_gateways() {
        let c = fetch_candidates("https://ipfs.io/ipfs/QmHash/1272.png");
        assert_eq!(c.len(), IPFS_GATEWAY_HOSTS.len());
        assert_eq!(c[0], "https://ipfs.io/ipfs/QmHash/1272.png");
        assert!(c.contains(&"https://dweb.link/ipfs/QmHash/1272.png".to_string()));
        // ipns path is also recognized
        assert_eq!(
            ipfs_path_suffix("https://ipfs.io/ipns/example.eth/logo.png"),
            Some("/ipns/example.eth/logo.png")
        );
    }

    #[test]
    fn non_ipfs_urls_are_tried_as_is() {
        let c = fetch_candidates("https://cdn.example.com/a.png");
        assert_eq!(c, vec!["https://cdn.example.com/a.png".to_string()]);
        assert_eq!(ipfs_path_suffix("https://cdn.example.com/a.png"), None);
        // subdomain-style ipns gateway has no path suffix → fetched as-is
        assert_eq!(
            ipfs_path_suffix("https://foo.ipns.dweb.link/logo.png"),
            None
        );
    }

    #[test]
    fn rewrites_ipfs_uris() {
        assert_eq!(
            to_http_url("ipfs://QmHash/pic.png").unwrap(),
            "https://ipfs.io/ipfs/QmHash/pic.png"
        );
        assert_eq!(
            to_http_url("ipfs://ipfs/QmHash").unwrap(),
            "https://ipfs.io/ipfs/QmHash"
        );
        assert_eq!(
            to_http_url("https://example.com/a.png").unwrap(),
            "https://example.com/a.png"
        );
        assert!(to_http_url("eip155:1/erc721:0xabc/1").is_err());
    }

    #[test]
    fn decodes_base64_data_uri() {
        // "PNG" magic as base64 → iVBO... not needed; use a tiny payload.
        let encoded = base64::engine::general_purpose::STANDARD.encode([0x89, b'P', b'N', b'G']);
        let uri = format!("image/png;base64,{encoded}");
        let bytes = decode_data_uri(&uri).unwrap();
        assert_eq!(sniff_ext(&bytes), Some("png"));
    }

    #[test]
    fn decodes_text_svg_data_uri() {
        let bytes = decode_data_uri("image/svg+xml,%3Csvg%3E%3C%2Fsvg%3E").unwrap();
        assert_eq!(String::from_utf8_lossy(&bytes), "<svg></svg>");
    }
}
