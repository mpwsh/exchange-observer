#!/bin/bash
scp -r Cargo.toml config.toml scylla scripts lib producer consumer scheduler farm:/home/farm/projects/exchange-observer/
