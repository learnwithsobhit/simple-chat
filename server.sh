#!/bin/bash
RUST_LOG=info  cargo run --bin server $1:$2
