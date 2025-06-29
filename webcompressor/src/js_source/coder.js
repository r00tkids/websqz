let U24_MAX = 0xFFFFFF;

let ArithmeticDecoder = () => {
    let self = {
        state: 0,
        low: 0,
        high: 0xFFFFFFFF,
        readPtr: 0,
    };

    return {
        decode: (p, input) => {
            if (p > U24_MAX) throw new Error("p > U24_MAX");
            if (self.high <= self.low) throw new Error("high <= low");

            let mid = self.low + ((self.high - self.low) * p);

            if (!(self.high > mid && mid >= self.low)) throw new Error("mid out of range");

            let bit = 0;
            if (self.state <= mid) {
                bit = 1;
                self.high = mid;
            } else {
                self.low = mid + 1;
            }

            while ((self.high ^ self.low) < 0x1000000) {
                self.low = self.low << 8;
                self.high = (self.high << 8) | 0xFF;
                if (self.readPtr >= input.length) {
                    return;
                }
                let c = input[self.readPtr];
                self.state = (self.state << 8) | c;
            }

            return bit;
        }
    };
};

let decompress = (data) => {
    let decoder = ArithmeticDecoder();
    let input = new Uint8Array(data);
    let output = [];

    for (let i = 0; i < input.length; i++) {
        let p = decoder.decode(input[i], input);
        if (p === undefined) {
            break; // End of input
        }
        output.push(p);
    }

    return new Uint8Array(output);
}