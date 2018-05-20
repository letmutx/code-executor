# code-executor

Code executor in Rust using docker

~~There are lot of bugs, I know. I will (hopefully) fix them someday~~. I did fix most and it is usable. Feel free to do whatever with it.

## Steps to run

You might have to pull images required, see Dockerfile

1. Install [rust](https://rustup.rs)
2. Install [docker](https://get.docker.com)
3. Do `cargo run`

You can use cURL. See sample json in `resources/<lang>/*.json`

`$ curl -v 'https://localhost:3000/execute' --data @file`
