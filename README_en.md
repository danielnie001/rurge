<div align="center">

<img src="docs/assets/rurge-icon.svg" width="128" height="128" alt="rurge icon">

# rurge

[中文](README.md) | **English**

</div>

---

### Introduction

rurge (**Ru**st + Su**rge**) is a cross-platform network proxy written in Rust. Its goal is to re-implement, phase by phase, the full feature set of [Surge](https://nssurge.com/) (macOS / iOS) while being:

- **Natively compatible with Surge's configuration format**: `.conf` profiles, `.sgmodule` modules, `RULE-SET` / `DOMAIN-SET` rule sets, `policy-path` subscriptions and managed profiles work as-is;
- **Compatible with Surge's scripting API**: existing `http-request` / `http-response` / `cron` / `event` / `dns` / `rule` / `generic` scripts run unchanged;
- **Compatible with Surge's HTTP API**: existing dashboards and automation tools can connect directly;
- **Core first, cross-platform**: one command-line daemon shared by Windows / Linux / macOS, with a desktop GUI planned for a later phase.

### Status

> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); the remaining outbound protocols, the group algorithms (`url-test` / `fallback` / `load-balance` / `smart` / `subnet`) and policy subscriptions are later phase-2 milestones.

See [docs/requirements.md](docs/requirements.md) (Chinese) for the full requirements, module breakdown, platform matrix and phased roadmap, and [docs/surge-compatibility-matrix.md](docs/surge-compatibility-matrix.md) for the item-by-item Surge compatibility checklist.

### Planned features

| Module              | Scope                                                                                                                                                                   | Phase |
| ------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----- |
| Profile             | Surge`.conf` parser, every `[General]` option, managed profiles, `.sgmodule` modules, requirement expressions, keystore                                           | 1 / 5 |
| Inbound             | HTTP / HTTPS proxy, SOCKS5, LAN sharing with authentication, system proxy (HTTP / SOCKS5 listeners, Basic auth, `proxy-restricted-to-lan` implemented, M3a; the three-platform system-proxy switch, `skip-proxy` conversion, and recovery on exit / crash implemented, M4b)                                                                                               | 1     |
| Rules               | Domain / IP / GEOIP / IP-ASN / HTTP / process / source & port / protocol & network / logical / script / rule sets / FINAL (rule engine / rule sets / GeoIP implemented, M2a)                                               | 1     |
| DNS                 | Plain DNS, DoH / DoT / DoQ / DoH3, local mapping, hijacking, fake IP, always-real-ip (plain DNS / DoH / DoT / `tcp://` / `[Host]` / system hosts implemented, M2b)     | 1 / 3 |
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP / SOCKS5 (including `socks5-tls`) TCP upstreams and `underlying-proxy` chains implemented, phase 2 / M1; Trojan (TCP; WebSocket transport) implemented, phase 2 / M2a; VMess (AEAD handshake, TCP; optionally TLS / WebSocket) and AnyTLS (TCP, session reuse) implemented, phase 2 / M2b) | 2     |
| Policy groups       | select / url-test / fallback / load-balance / smart / subnet, policy including and subscriptions, latency tests (a `select` group's choice can be read and switched over the API, phase 2 / M1)                                                         | 2     |
| Enhanced mode       | Virtual interface (Wintun / tun / utun), UDP, included and excluded routes, process identification, subnet settings                                                     | 3     |
| HTTP processing     | MITM (HTTPS decryption), URL / header / body rewrite, Map Local, request viewer and capture                                                                             | 4     |
| Scripting & modules | JavaScript engine, full Surge scripting API, module system, information panels                                                                                          | 5     |
| API & tools         | Surge-compatible HTTP API, web dashboard, logbook, latency / benchmark tests, CLI (phase-1 endpoints and `rurge reload/stop/status` implemented, M4a; `rurge service install/uninstall` (basic) implemented, M4b)                   | 6     |
| Advanced networking | Gateway mode, DHCP server, port forwarding, built-in Snell / MTProto servers                                                                                            | 7     |
| Desktop GUI         | Cross-platform desktop client, URL scheme                                                                                                                               | 8     |

### Roadmap

1. **Phase 0** Requirements and documentation (current)
2. **Phase 1** Core skeleton: config parser, HTTP / SOCKS5 inbound, DIRECT / REJECT, rule engine, DNS, CLI, HTTP API skeleton, system proxy, service install (phase-1 HTTP API endpoints and `rurge reload/stop/status` are done, M4a; the system proxy and `rurge service install/uninstall` (basic) are done, M4b)
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done)
4. **Phase 3** Enhanced mode (TUN), fake IP, process identification
5. **Phase 4** HTTP engine: MITM, rewrites, Map Local, capture
6. **Phase 5** Scripting engine and module system
7. **Phase 6** Full HTTP API compatibility, web dashboard, testing and diagnostic tools
8. **Phase 7** Gateway / DHCP / port forwarding / built-in servers
9. **Phase 8** Desktop GUI

Near-term non-goals: iOS / tvOS builds, Surge Ponte (depends on iCloud), Apple-only UI. Tailscale integration is a long-term item pending evaluation.

### Quick start (planned)

> `rurge check`, `rurge rule match`, `rurge dns lookup` and `rurge run` (HTTP / SOCKS5 proxy, DIRECT / REJECT, and `http` / `https` / `socks5` / `socks5-tls` / `trojan` / `vmess` / `anytls` upstreams — all optionally wrapped in Shadow TLS — including `underlying-proxy` chains) work today; the group algorithms arrive in later phase-2 milestones. The HTTP API and `rurge reload` / `stop` / `status` are available (see [docs/api/phase1.md](docs/api/phase1.md); phase-2 additions in [docs/api/phase2.md](docs/api/phase2.md)). `rurge run --system-proxy` points the system proxy at rurge and restores it on exit, or at the next start after a crash; `rurge service install | uninstall [--user] [--dry-run]` registers or removes automatic startup (systemd / launchd / a Windows scheduled task). On macOS, `networksetup` usually needs an administrator account; whether `sudo` is required has not been verified on real hardware yet (see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)) — on failure, rurge passes the tool's error through as-is. `rurge run` also takes rurge-specific runtime options — `--idle-timeout`, `--request-log-size`, `--watch` (hot reload), `--log-file` (daily rotation) — as CLI flags / env vars only, never written into the Surge profile.

```bash
# Build
cargo build --release

# Validate a profile
rurge check -c config.conf

# Test offline which rule a request would hit and which policy it resolves to (no daemon needed)
rurge rule match -c surge.conf www.example.com --explain

# Resolve a name offline through the profile's DNS settings
rurge dns lookup -c surge.conf www.example.com --trace

# Run
rurge run -c config.conf

# Run and point the system proxy at rurge (restored on exit, or at the next start after a crash)
rurge run -c config.conf --system-proxy

# Preview what registering automatic startup would do (drop --dry-run to install)
rurge service install -c config.conf --user --dry-run
```

Profiles use the Surge format directly; see the example in the Chinese section above.

### Documentation

- [Requirements (PRD, Chinese)](docs/requirements.md)
- [Surge compatibility matrix (Chinese)](docs/surge-compatibility-matrix.md)
- Per-phase design specs and implementation plans will live in `docs/superpowers/specs/` and `docs/superpowers/plans/`

### Development

- Toolchain: Rust stable via `rustup`; `cargo fmt`, `cargo clippy`, `cargo test`
- Workflow: each phase starts with a design spec, then an implementation plan, then implementation with tests
- Please read the requirements document and open an issue before large changes

### Disclaimer

- rurge is an independent open-source project and is not affiliated with NSSurge, the developer of Surge. "Surge" is a trademark of its owner and is used here only to describe compatibility.
- This project provides no proxy servers or network services. Use it in accordance with the laws of your jurisdiction.
- MITM (HTTPS decryption) and similar features must only be used on devices and traffic you own or are authorised to inspect.
- GEOIP / IP-ASN rules default to MaxMind's GeoLite2 database: This product includes GeoLite2 data created by MaxMind, available from <https://www.maxmind.com>.

### License

[MIT](LICENSE)
