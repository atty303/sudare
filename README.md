<img width="266" alt="black" src="https://user-images.githubusercontent.com/316079/210174802-20196383-f5e4-46ab-8d14-ecf38f0d3c75.png#gh-dark-mode-only">
<img width="266" alt="white" src="https://user-images.githubusercontent.com/316079/210174805-327ffbee-e147-446a-813d-cd23b1f36670.png#gh-light-mode-only">

Manage Procfile-based applications with terminal multiplexer

![2023-01-01_23-59-02](https://user-images.githubusercontent.com/316079/210175163-e47e973f-d470-4946-bfba-449e09a4a904.gif)

## Example Procfile

```
ping[google]: ping 8.8.8.8
ping[cloudflare]: ping 1.1.1.1
time: while true; do date; sleep 1; done
echo[hello]: echo Hello, world!
echo[good-by]: echo Good-bye world!
boolean[true]: true
boolean[false]: false
256-color: curl -s https://gist.githubusercontent.com/HaleTom/89ffe32783f89f403bba96bd7bcd1263/raw/e50a28ec54188d2413518788de6c6367ffcea4f7/print256colours.sh | bash
```

# Features

* It is written in Rust, so it is small and light.
* Output multiplexing makes it easy to see the output of individual processes.
* You can group processes and activate one of them.
* There is no ability to scale the number of processes.

# Install

## Supported Architectures

| Architecture Triple      | Operating System      |
| ------------------------ | --------------------- |
| aarch64-apple-darwin     | macOS (Apple Silicon) |
| x86_64-apple-darwin      | macOS (Intel)         |
| x86_64-unknown-linux-gnu | Linux (Intel)         |

## via Homebrew

```
brew install atty303/tap/sudare
```

# Usage

```
sudare <procfile>
```

## Keymap

| Key     | Function                       |
| ------- | ------------------------------ |
| ESC     | Exit                           |
| n, DOWN | Next process group             |
| p, UP   | Previous process group         |
| 0-9     | Select active process in group |
| j       | Scroll up                      |
| k       | Scroll down                    |

## Procfile extension

You can group processes and activate one of them.

```
group-name[process-name-1]: echo foo
gorup-name[process-name-2]: echo bar
```
