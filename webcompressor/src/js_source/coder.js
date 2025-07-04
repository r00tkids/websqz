let ArithmeticDecoder = (input, offset) => {
    let self = {
        state: 0,
        low: 0,
        high: 0xFFFFFFFF,
        readPtr: offset,
    };

    for (let i = 0;i < 4;++i) {
        let c = self.readPtr >= input.byteLength ? 0 : input[self.readPtr++];
        self.state = ((self.state << 8) | c) >>> 0;
    }

    return {
        decode: (p) => {
            if (p < 0. && p >= 1.) throw new Error("probability out of range");
            if (self.high <= self.low) throw new Error("high <= low");

            let mid = (self.low + (self.high - self.low) * p) >>> 0;
            if (mid >= self.high) {
                // We loose some precision to prevent overflow
                // Unlikely to happen in practice
                mid = self.high - 1;
            }

            if (!(self.high > mid && mid >= self.low)) throw new Error("mid out of range");

            let bit = 0;
            if (self.state <= mid) {
                bit = 1;
                self.high = mid;
            } else {
                self.low = (mid + 1) >>> 0;
            }

            while (((self.high ^ self.low) >>> 0) < (1 << 24)) {
                self.low = (self.low << 8) >>> 0;
                self.high = ((self.high << 8) | 0xFF) >>> 0;
                let c = self.readPtr >= input.byteLength ? 0 : input[self.readPtr++];    
                self.state = ((self.state << 8) | c) >>> 0;
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