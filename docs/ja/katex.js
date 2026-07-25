(function () {
    function initKaTeX() {
        var css = document.createElement("link");
        css.rel = "stylesheet";
        css.href = "https://cdn.jsdelivr.net/npm/katex@0.16.8/dist/katex.min.css";
        document.head.appendChild(css);

        var script = document.createElement("script");
        script.src = "https://cdn.jsdelivr.net/npm/katex@0.16.8/dist/katex.min.js";
        script.onload = function () {
            var autoRender = document.createElement("script");
            autoRender.src = "https://cdn.jsdelivr.net/npm/katex@0.16.8/dist/contrib/auto-render.min.js";
            autoRender.onload = function () {
                window.renderMathInElement(document.body, {
                    delimiters: [
                        { left: "$$", right: "$$", display: true },
                        { left: "$", right: "$", display: false },
                        { left: "\\(", right: "\\)", display: false },
                        { left: "\\[", right: "\\]", display: true }
                    ],
                    throwOnError: false
                });
            };
            document.head.appendChild(autoRender);
        };
        document.head.appendChild(script);
    }

    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", initKaTeX);
    } else {
        initKaTeX();
    }
})();
