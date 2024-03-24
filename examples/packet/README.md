# Acts Package

`acts` package is based on [`wasmtime`](<https://github.com/bytecodealliance/wasmtime>) runtime to build.

The example includes a pack1 exmpale to build wasm wit component. 

Before build it, please install `cargo-component`. for more detail, please see [`component-model`](<https://component-model.bytecodealliance.org/language-support/rust.html>)

```bash
> cargo install cargo-component

```

To build the `pack1`

```bash
> cd pack1
> cargo component build
```


To run the example
```bash
> cargo run --example package --features "wit"
```