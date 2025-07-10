{{{decompressor_source}}}

p.slice(o).arrayBuffer().then(b => {
    let d = decompress(model, new Uint8Array(b));
    let s = new TextDecoder().decode(d);
    document.body.innerHTML = "";
    eval(s);
});
