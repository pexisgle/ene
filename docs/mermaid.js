document.addEventListener("DOMContentLoaded", function () {
    const codeBlocks = document.querySelectorAll("pre code.language-mermaid");
    codeBlocks.forEach(function (block) {
        const pre = block.parentElement;
        const div = document.createElement("div");
        div.className = "mermaid";
        div.textContent = block.textContent;
        pre.parentNode.replaceChild(div, pre);
    });
    import("https://cdn.jsdelivr.net/npm/mermaid@10/dist/mermaid.esm.min.mjs").then((mermaid) => {
        mermaid.default.initialize({ startOnLoad: true, theme: "dark" });
    });
});
