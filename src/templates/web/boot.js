{{{decompressor_source}}}

p.slice(o).arrayBuffer().then(b => {
    d = decompress(model, new Uint8Array(b));
    s = new TextDecoder().decode(d);
    document.body.innerHTML = "";
    eval(s);
});
