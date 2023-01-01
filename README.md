# <img width="266" alt="black" src="https://user-images.githubusercontent.com/316079/210174802-20196383-f5e4-46ab-8d14-ecf38f0d3c75.png#gh-dark-mode-only">
# <img width="266" alt="white" src="https://user-images.githubusercontent.com/316079/210174805-327ffbee-e147-446a-813d-cd23b1f36670.png#gh-light-mode-only">

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
