let U24Max = 0xffffff;

let NOrderByteHashMap = HashMap(28, 4, { prob: U24Max >>> 1, count: 0 }, (view, value) => {
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
    let bitMaskLow = 0;
    let bitMaskHigh = 0;
    let bitCtx = 1;
    let prevBytesLow = 0;
    let prevBytesHigh = 0;
    for (let i = 0; i < 4; i++) {
        bitMaskLow |= ((byteMask >>> i) & 1) * (0xff << (i * 8));
    }
    for (let i = 4; i < 8; i++) {
        bitMaskHigh |= ((byteMask >>> i) & 1) * (0xff << ((i-4) * 8));
    }

    return {
        pred: () => {
            return NOrderByteHashMap.get(ctx ^ bitCtx).prob / U24Max;
        },
        learn: (bit) => {
            let value = NOrderByteHashMap.get(ctx ^ bitCtx);
            if (value.count < maxCount) {
                value.count++;
            }
            let countPow = Math.pow(value.count, 0.72) + 0.19;
            value.prob += U24Max * ((bit - (value.prob / U24Max)) / countPow);
            NOrderByteHashMap.set(ctx ^ bitCtx, value);

            bitCtx = (bitCtx << 1) | bit;
            if (bitCtx >= 256) {
                let currentByte = bitCtx & 0xff;
                bitCtx &= byteMask;

                prevBytesHigh = (prevBytesHigh << 8) | (prevBytesLow >>> 24);
                prevBytesLow = (prevBytesLow << 8) | currentByte;

                let maskedByteLow = prevBytesLow & bitMaskLow;
                let maskedByteHigh = prevBytesHigh & bitMaskHigh;
                ctx = (hash(maskedByteHigh, 3) * 9 + hash(maskedByteLow, 3)) * magicNum;

                bitCtx = 1;
            }
        },
    };
};