# acts-package-http

The acts http package plugin for acts. 

## Installation


```bash
cargo add acts-package-http
```

## Start

```rust,no_run
use acts::Engine;
use acts_package_http::HttpPackage;

#[tokio::main]
async fn main() {
    let engine = Engine::builder()
        .add_pacakge::<HttpPackage>()
        .start();
}
```

## Example

```yml
name: http example
id: http-example
inputs:
  key1: 1
  key2: 2
steps:
  - name: http step
    uses: acts.core.http
    params:
      url: http://127.0.0.1:1234/hello
      method: GET
      # params from workflow.inputs
      params: 
        - key: key1
          value: '${{ key1 }}'
        - key: key2
          value: '${{ key2 }}'
  - name: http step 2
    uses: acts.core.http
    params:
      url: http://127.0.0.1:1234/world
      method: POST
      content-type: json
      # body data from prev http response data
      body:
        data: '${{ $inputs().data }}'

```

## Configuration

The package reads an optional `[http]` section from the engine config
(`acts.toml`), or `HttpPackage::from_config` when building it directly:

```toml
[http]
# Egress allowlist. When non-empty only these hosts can be requested;
# "*.example.com" matches subdomains but not example.com itself.
allowed-hosts = ["api.example.com", "*.example.org"]
# Opt-in for internal/on-prem endpoints. Cloud metadata addresses stay
# blocked even when this is true.
allow-private-addresses = false
# Hard cap on a response body; larger bodies fail the act.
max-response-bytes = 67108864
# Connect timeout; 0 disables it.
connect-timeout-ms = 10000
# Whole-request timeout; omitted or 0 disables it.
timeout-ms = 30000
```

## Egress policy

Every target is validated before connecting and again at DNS resolution /
connection time:

- only `http` and `https` are accepted;
- with `allowed-hosts` set, the host must match an entry;
- loopback, RFC1918, link-local, carrier-grade NAT and IPv6 unique-local
  addresses are rejected unless `allow-private-addresses = true`;
- cloud metadata endpoints (169.254.169.254, 100.100.100.200,
  fd00:ec2::254) are always rejected;
- redirects are re-validated, so a permitted URL cannot bounce the client
  to an internal target.

The request uses one package-level async client, so connections and TLS
sessions are reused. `timeout-ms`, `connect-timeout-ms` and
`max-response-bytes` bound how long an act can hang and how much a single
response can allocate; without them a remote server could stream
indefinitely.

Local example servers (e.g. `http://127.0.0.1:1234`) need
`allow-private-addresses = true`.

An ambient proxy (`HTTP_PROXY`/`HTTPS_PROXY`) sends requests through the
proxy, so the address-range checks apply to the proxy rather than the
target; unset the proxy or use `NO_PROXY` when it can reach internal
networks.
