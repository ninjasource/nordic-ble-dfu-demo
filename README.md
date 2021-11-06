# nordic-ble-dfu-demo

Nordic buttonless bluetooth Device Firmware Update demo in Rust. This bootloader is considered secure because you sign your application binaries with a private key (that you generate) and the bootloader uses its public key (that you also generate) to verify the signature of your app. 
This way anyone can attempt to update the firmware but the bootloader will reject anything not signed by you. Therefore it is not necessary to use bonding, shared secrets or control access to the dfu service itself.

This demo targets the nRF52840 mcu but should work with other nordic mcu's by tweaking things.

## How to build the secure bootloader (written in c)

First, you need to build your own bootloader. The easiest way to do this is to download the Nordic nRF5_SDK_17.x.x as well as the segger embedded studio also supplied by Nordic (i.e. it is free to use) and just use the example secure bootloader Nordic supplies. 

Also download the `nrfutil` tool which you can use to generate the signing key.

Generate a private key with the following command: `nrfutil keys generate private.key`
Generate a public key from that private key as follows: `nrfutil keys display --key pk --format code private.key --out_file public_key.c`

Open the `~/nordic/nRF5_SDK_17.0.2/examples/dfu/secure_bootloader/pca10100_s140_ble_debug/ses/secure_bootloader_ble_s140_pca10100_debug.emProject` solution in Segger Embedded Studio. 
> Not sure why I chose the debug project but it works.
Copy the contents of `public_key.c` into `~/nordic/nRF5_SDK_17.0.2/examples/dfu/dfu_public_key.c` (wherever you saved your sdk to)
Build the solution and locate the output hex `~/nordic/nRF5_SDK_17.0.2/examples/dfu/secure_bootloader/pca10100_s140_ble_debug/ses/Output/Release/Exe/secure_bootloader_ble_s140_pca10100_debug.hex`. 
This is your secure bootloader with all it's associated flash settings. You can change other things like the advert name (the default is `DftTarg`) but whatever.

## Flash your the bootloader and softdevice to your mcu

Using the Nordic `nRF Connect` app on your PC flash the following files in the `dfu` folder: `s140_nrf52_7.3.0_softdevice.hex` and `secure_bootloader_ble_s140_pca10100.hex` (the secure_bootloader should be the one you built above)

## Build your rust app and create a signed package

Navigate the the `dfu` folder and run `./build_app.sh`

This will generate a zip file in the `./dfu/target` folder. Copy this file to your phone and load up the mobile phone version of the `nRF Connect` app. 
Now, recall that the only software on your MCU is the softdevice and the bootloader.
With a bit of luck, when you scan for devices you will see a `DfuTarg` device which is the bootloader. 
Connect to it and navigate to the Dfu tab. Select your file and update the firmware.
Now, when you scan for bluetooth devices, `Dave` should come up. This is your app and it also works with the `nRF Connect` app. It basically offloads the work onto the bootloader by restarting the device in dfu mode.

