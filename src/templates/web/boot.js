{{{decompressor_source}}}

p.slice(o).arrayBuffer().then(b => {
    a = new Uint8Array(b);
    d = decompress(model, a);
    {{{files_map}}}
    s = new TextDecoder().decode(d);
    document.body.innerHTML = "";
    eval(s);
});
