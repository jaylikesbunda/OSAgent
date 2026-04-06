---
name: word
description: "Create and edit Microsoft Word documents via python-docx"
emoji: "📄"
requires:
  bins: ["python3"]
config:
  - name: OUTPUT_DIR
    type: string
    description: "Default directory for generated Word documents"
    required: false
actions:
  - name: create_doc
    description: "Create a new Word document with a title and optional paragraphs"
    type: script
    script: "scripts/create_doc.py"
    parameters:
      - name: title
        type: string
        description: "Document heading title"
        required: true
      - name: paragraphs
        type: string
        description: "Comma-separated paragraph texts"
        required: false
      - name: filename
        type: string
        description: "Output filename (default: document.docx)"
        required: false
---
# Microsoft Word Skill

Create and edit Microsoft Word documents using Python.

## Runtime Actions

- `create_doc(title, paragraphs?, filename?)` creates a new `.docx` file with a heading and paragraphs.

## Setup

1. Install python-docx: `pip install python-docx`
2. Create documents with natural language like "Create a Word doc called report.docx with an intro paragraph"
