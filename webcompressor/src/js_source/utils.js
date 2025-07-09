let U24Max = 0xffffff;
let U32Max = 0xffffffffn;
let U64Max = 0xffffffffffffffffn;

let probStretch = (prob) => {
    return Math.log(prob / (1. - prob));
};
let probSquash = (prob) => {
    return 1. / (1. + Math.exp(-prob));
};