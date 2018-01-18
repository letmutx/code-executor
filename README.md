# code-executor
Code executor in Rust using docker

There are ~~lot of bugs, I know. I will (hopefully) fix them someday~~. Feel free to do whatever with it.

## Steps to run

1. Install [rust](https://rustup.rs)
2. Install [docker](https://get.docker.com)
3. Do `cargo run`

Copy this json to a file, 

```json
{
	"code": "#include <stdio.h>\nint main(void) { printf(\"Hello world\"); return 0; }",
	"lang": "C"
}
```

You can use cURL

`$ curl -v 'https://localhost:3000/execute' --data @file`
