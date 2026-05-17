document.body.innerHTML = "<h1>Hello, rootsqz!</h1>";

let glsl = document.createElement("p").textContent = new TextDecoder().decode(rsqz.files["bundled.glsl"]);
console.log("GLSL content:", glsl);