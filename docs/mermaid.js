(function () {
    function initMermaid() {
        var script = document.createElement("script");
        script.src = "https://cdn.jsdelivr.net/npm/mermaid@10/dist/mermaid.min.js";
        script.onload = function () {
            window.mermaid.initialize({
                startOnLoad: false,
                theme: "dark",
                securityLevel: "loose"
            });
            renderDiagrams();
        };
        document.head.appendChild(script);
    }

    function renderDiagrams() {
        var blocks = document.querySelectorAll("pre code.language-mermaid, pre code.mermaid");
        blocks.forEach(function (block) {
            var pre = block.parentElement;
            var div = document.createElement("div");
            div.className = "mermaid";
            div.textContent = block.textContent;
            pre.parentNode.replaceChild(div, pre);
        });
        window.mermaid.run({ querySelector: ".mermaid" });
    }

    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", initMermaid);
    } else {
        initMermaid();
    }
})();
