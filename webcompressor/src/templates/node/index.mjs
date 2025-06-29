import fs from 'fs';

{{{decompressor_source}}}

fs.readFileSync('{{{input_file}}}', (err, data) => {
    if (err) {
        console.error('Error reading file:', err);
        return;
    }

    fs.writeFileSync('{{{output_file}}}', decompress(data));
});

