from __future__ import annotations

import re
import unittest
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parents[1]
MARKDOWN_LINK = re.compile(r"(?<!!)\[[^]]*]\(([^)]+)\)")


class DocumentationTest(unittest.TestCase):
    def test_every_japanese_document_has_an_english_peer(self) -> None:
        self.assertTrue((PROJECT_ROOT / "README_en.md").is_file())
        for japanese_path in (PROJECT_ROOT / "docs").glob("*.md"):
            if japanese_path.stem.endswith("_en"):
                continue
            english_path = japanese_path.with_name(f"{japanese_path.stem}_en.md")
            self.assertTrue(english_path.is_file(), english_path)

    def test_local_markdown_links_resolve(self) -> None:
        documents = [PROJECT_ROOT / "README.md", PROJECT_ROOT / "README_en.md"]
        documents.extend((PROJECT_ROOT / "docs").glob("*.md"))
        for document in documents:
            for raw_target in MARKDOWN_LINK.findall(
                document.read_text(encoding="utf-8")
            ):
                target = raw_target.split(maxsplit=1)[0].strip("<>")
                if target.startswith(("#", "https://", "http://", "mailto:")):
                    continue
                linked_path = (document.parent / target.split("#", 1)[0]).resolve()
                with self.subTest(document=document.name, target=target):
                    self.assertTrue(linked_path.exists(), linked_path)
