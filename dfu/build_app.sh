cd ..
cargo objcopy --release -- -O ihex ./dfu/target/nordic-ble-dfu-demo.hex

cd ./dfu
nrfutil pkg generate --hw-version 52 --sd-req 0x0123 --sd-id 0x0123 --application-version 1 --application ./target/nordic-ble-dfu-demo.hex --key-file private.key ./target/nordic-ble-dfu-demo.zip

