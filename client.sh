#!/bin/bash
RUST_LOG=info cargo run --bin client $1:$2 $3
