import json
import sys
from pathlib import Path

from playwright.sync_api import sync_playwright


def main() -> None:
    output = Path(sys.argv[1] if len(sys.argv) > 1 else "artifacts/desktop-probe.png").resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as playwright:
        browser = playwright.chromium.connect_over_cdp("http://127.0.0.1:9223")
        pages = [page for context in browser.contexts for page in context.pages]
        matches = [page for page in pages if page.title() == "UCode Portable"]
        if len(matches) != 1:
            raise RuntimeError(f"expected one UCode page, found {len(matches)}")
        page = matches[0]
        page.wait_for_load_state("domcontentloaded")
        page.wait_for_timeout(500)
        page.screenshot(path=str(output), full_page=True)
        print(json.dumps({
            "title": page.title(),
            "url": page.url,
            "buttons": page.get_by_role("button").all_text_contents(),
            "textareas": page.locator("textarea").count(),
            "body": page.locator("body").inner_text()[:8000],
            "screenshot": str(output),
        }, ensure_ascii=False, indent=2))
        browser.close()


if __name__ == "__main__":
    main()
