{{{decompressor_source}}}

document.body.innerHTML = "";
p.slice(o).arrayBuffer().then(b => {
    a = new Uint8Array(b);
    d = decompress(model, a);
    {{{files_map}}}
    s = new TextDecoder().decode(d);
    eval(s);
});
