//! `GET /v1/geoip?ip=<address>`: what the databases behind the `GEOIP` and
//! `IP-ASN` rules say of an address, and the build times of the databases.

use crate::App;
use crate::error::{ApiError, ApiResult, query_params};
use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

#[derive(Deserialize)]
pub struct GeoIpQuery {
    pub ip: String,
}

/// What has no result is `null`, a database that is not there included. The
/// dates are the build times the databases carry, in UNIX seconds.
#[derive(Serialize)]
pub struct GeoIpJson {
    pub address: String,
    pub country: Option<String>,
    pub asn: Option<u32>,
    pub organization: Option<String>,
    #[serde(rename = "geoip-db-date")]
    pub geoip_db_date: Option<u64>,
    #[serde(rename = "asn-db-date")]
    pub asn_db_date: Option<u64>,
}

pub async fn geoip(
    State(app): State<App>,
    query: Result<Query<GeoIpQuery>, QueryRejection>,
) -> ApiResult<Json<GeoIpJson>> {
    let query = query_params(query)?;
    let ip: IpAddr = query
        .ip
        .parse()
        .map_err(|_| ApiError::bad_request("ip is not a valid IP address"))?;
    // an IPv4-mapped address is the IPv4 address it names, here as everywhere
    let ip = ip.to_canonical();
    let view = app.engine.geoip_view(ip);
    Ok(Json(GeoIpJson {
        address: ip.to_string(),
        country: view.country,
        asn: view.asn,
        organization: view.organization,
        geoip_db_date: view.country_db_date,
        asn_db_date: view.asn_db_date,
    }))
}
