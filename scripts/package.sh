#!/bin/bash

set -e
mkdir -p temp_image
sudo mount -o loop $IMAGE temp_image

sudo mkdir -p temp_image/boot
sudo cp $KERNEL_IMAGE temp_image/boot/kernel

sudo umount temp_image
