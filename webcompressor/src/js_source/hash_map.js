let HashMap = (pow2Size, itemSize, defaultValue, itemEncoder, itemDecoder) => {
    let data = new ArrayBuffer(2 ** pow2Size * itemSize);
    let mask = (1 << pow2Size) - 1;

    for (let i = 0; i < 2 ** pow2Size; i++) {
        itemEncoder(new DataView(data, i * itemSize), defaultValue);
    }
    console.log(`HashMap created with size: ${2 ** pow2Size} items, item size: ${itemSize} bytes`);
    console.log(`Total size: ${(2 ** pow2Size * itemSize) / (1024 * 1024)} MiB`);

    return {
        get: (key) => {
            return itemDecoder(new DataView(data, (key & mask) * itemSize));
        },
        set: (key, value) => {
            itemEncoder(new DataView(data, (key & mask) * itemSize), value);
        }
    };
}

function hash(value, shift) {
    const K_MUL = 0x9E35A7BDn;
    value ^= value >> BigInt(shift);
    return ((K_MUL * value) & 0xffffffffn) >> BigInt(shift);
}