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

The request uses one package-level async client, so connections and TLS
sessions are reused. Set `timeout-ms` for an optional total request timeout
(including reading the response body). If it is omitted, the request has no
timeout and can wait indefinitely for a remote server.
