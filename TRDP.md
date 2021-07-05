# prellblock-trdp

## get started

### Use Docker-Container

Using the VSCode *Remote - Containers* extension and run it in docker container.

### build
```
cargo build
```


### test trdp-tcp-listener
```
cd hello_world
cargo run --bin md_listener
```
```
cd trdp
./test_mdSingle -o 127.0.0.1 -t 127.0.0.1
```


### test trdp-tcp-sender

```
cd trdp
./test_mdSingle -o 127.0.0.1 -r
```

```
export RUST_LOG=info
cd hello_world
cargo run --bin md_sender -- 17225
```

### test communication
```
cd hello_world
cargo run --bin md_listener
```
```
cd hello_world
cargo run --bin md_sender -- 17225
cargo run --bin md_sender -- 8080
```



### create certificates
```
cd genesis-wizard
cargo run --bin genesis-wizard
```
follow the steps and create 4 RPU, 1 admim and 1 sensor Account



### run node
genesis file is only needed for the first run
```
cargo run --bin prellblock -- config/emily/emily.toml config/genesis/genesis.yaml
cargo run --bin prellblock -- config/james/james.toml config/genesis/genesis.yaml
cargo run --bin prellblock -- config/percy/percy.toml config/genesis/genesis.yaml
cargo run --bin prellblock -- config/thomas/thomas.toml config/genesis/genesis.yaml
```


### add transaction
```
cargo run --bin prellblock-client -- config/temperature-1/temperature-1.key  127.0.0.1:3130 set hello world
```

### set log level
default is warning
```
export RUST_LOG=info
```
