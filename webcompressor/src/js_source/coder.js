let U24_MAX = 0xFFFFFF;
let U32_MAX = 0xFFFFFFFFn;

let ArithmeticDecoder = (input, offset) => {
    let self = {
        state: 0n,
        low: 0n,
        high: 0xFFFFFFFFn,
        readPtr: offset,
    };

    for (let i = 0;i < 4;++i) {
        let c = BigInt(self.readPtr >= input.byteLength ? 0 : input[self.readPtr++]);
        self.state = (self.state << 8n) | c;
    }

    return {
        decode: (p) => {
            if (p > U24_MAX) throw new Error("p > U24_MAX");
            if (self.high <= self.low) throw new Error("high <= low");

            let mid = BigInt((Number(self.low) + (Number(self.high - self.low) * p)) >>> 0);

            if (!(self.high > mid && mid >= self.low)) throw new Error("mid out of range");

            let bit = 0;
            if (self.state <= mid) {
                bit = 1;
                self.high = mid;
            } else {
                self.low = (mid + 1n) & U32_MAX;
            }

            while ((self.high ^ self.low) < 0x1000000n) {
                self.low = (self.low << 8n) & U32_MAX;
                self.high = ((self.high << 8n) | 0xFFn) & U32_MAX;
                let c = BigInt(self.readPtr >= input.byteLength ? 0 : input[self.readPtr++]);    
                self.state = ((self.state << 8n) | c) & U32_MAX;
            }

            return bit;
        }
    };
};

let decompress = (model, data) => {
    let outputSize = new DataView(data.buffer).getUint32(0);
    let decoder = ArithmeticDecoder(data, 4);
    let output = [];

    console.log("Output size:", outputSize);
    for (let byteIdx = 0;byteIdx < outputSize;++byteIdx) {
        let byte = 0;
        for (let i = 0;i < 8;++i) {
            let prob = probSquash(model.pred());
            let bit = decoder.decode(prob);
            model.learn(bit, bit - prob);
            byte = (byte << 1) | bit;
        }

        output.push(byte);

        if (byteIdx % 1024*10 === 0) {
            console.log(`Decoded ${byteIdx} bytes`);
        }
    }

    return new Uint8Array(output);
};