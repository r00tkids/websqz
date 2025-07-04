import fs from 'fs';

{{{decompressor_source}}}

fs.readFile('{{{input_file}}}', (err, data) => {
    if (err) {
        console.error('Error reading file:', err);
        return;
    }

    console.log(data.buffer);

    fs.writeFileSync('{{{output_file}}}', decompress(model, new Uint8Array(data.buffer)));
});

