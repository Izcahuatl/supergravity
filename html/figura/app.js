(function () {
    const container = document.getElementById('library-list');

    function renderError(message) {
        container.innerHTML = `<p class="empty-state">${message}</p>`;
    }

    function renderCard(lib) {
        const doc = lib.doc
            ? `<div class="doc-block">${lib.doc}</div>`
            : '';

        return `
            <article class="library-card">
                <h2 class="library-name">${escapeHtml(lib.name)}</h2>
                <p class="library-description">${escapeHtml(lib.description || '')}</p>
                ${doc}
                <a
                    href="libs/${encodeURIComponent(lib.filename)}"
                    download
                    class="btn btn-download"
                >
                    Download ${escapeHtml(lib.filename)}
                </a>
            </article>
        `;
    }

    function escapeHtml(text) {
        return String(text)
            .replace(/&/g, '&amp;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;')
            .replace(/"/g, '&quot;')
            .replace(/'/g, '&#039;');
    }

    async function init() {
        try {
            const response = await fetch('libraries.json');
            if (!response.ok) {
                throw new Error(`Could not load library list (${response.status})`);
            }
            const data = await response.json();
            const libraries = Array.isArray(data) ? data : data.libraries;

            if (!Array.isArray(libraries) || libraries.length === 0) {
                renderError('No libraries listed yet. Check back soon!');
                return;
            }

            container.innerHTML = libraries.map(renderCard).join('');
        } catch (err) {
            console.error(err);
            renderError('Something went wrong while loading the libraries.');
        }
    }

    init();
})();
