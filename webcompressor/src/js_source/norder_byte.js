let U24Max = 0xffffff;
let U64Max = 0xffffffffffffn;

let NOrderByteHashMap = HashMap(10/*28*/, 4, { prob: U24Max >> 1, count: 0 }, (view, value) => {
    view.setUint32(0, value.prob & U24Max | (value.count << 24));
}, (view) => {
    return {
        prob: view.getUint32(0) & U24Max,
        count: view.getUint32(0) >>> 24
    };
});

let NOrderByte = (byteMask) => {
    let ctx = 0;
    let maxCount = 255;
    let bitMask = 0n;
    let bitCtx = 1;
    let prevBytes = 0n;
    let magicNum = hash(BigInt(byteMask), 2);

    for (let i = 0; i < 8; i++) {
        bitMask |= BigInt(((byteMask >>> i) & 1) * (0xff << (i * 8)));
    }

    return {
        pred: () => {
            return probStretch(NOrderByteHashMap.get(ctx ^ bitCtx).prob / (U24Max + 1));
        },
        learn: (bit) => {
            let value = NOrderByteHashMap.get(ctx ^ bitCtx);
            if (value.count < maxCount) {
                value.count++;
            }
            let countPow = Math.pow(value.count, 0.72) + 0.19;
            value.prob += (U24Max * ((bit - (value.prob / U24Max)) / countPow)) | 0;
            NOrderByteHashMap.set(ctx ^ bitCtx, value);

            bitCtx = (bitCtx << 1) | bit;
            if (bitCtx >= 256) {
                let currentByte = bitCtx & 0xff;

                prevBytes = ((prevBytes << 8n) | BigInt(currentByte)) & U64Max;

                let maskedBytes = prevBytes & bitMask;
                ctx = Number(((hash(maskedBytes >> 32n, 3) * 9n + hash(maskedBytes, 3)) * magicNum) & 0x7fffffffn);

                bitCtx = 1;
            }
        },
    };
};