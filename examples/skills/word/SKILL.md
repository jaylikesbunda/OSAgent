---
name: word
description: "Microsoft Word document operations via python-docx"
emoji: "📄"
requires:
  bins: ["python3"]
---
# Microsoft Word Skill

Create and edit Microsoft Word documents using Python.

## Commands

### Create Document
```bash
python3 << 'EOF'
from docx import Document
doc = Document()
doc.add_heading("{{ skill.env.TITLE | default: 'Untitled' }}", 0)
{% for para in skill.env.PARAGRAPHS | split: ',' %}
doc.add_paragraph("{{ para }}")
{% endfor %}
doc.save("{{ skill.env.OUTPUT_PATH }}")
print("Document saved to {{ skill.env.OUTPUT_PATH }}")
EOF
```

### Add Paragraph
```bash
python3 << 'EOF'
from docx import Document
doc = Document("{{ skill.env.FILE_PATH }}")
doc.add_paragraph("{{ skill.env.TEXT }}")
doc.save("{{ skill.env.FILE_PATH }}")
print("Paragraph added")
EOF
```

## Configuration

| Variable | Description | Required |
|----------|-------------|----------|
| `PYTHON_BIN` | Path to python3 (default: python3) | No |

## Setup

1. Install python-docx: `pip install python-docx`
2. Create documents with natural language like "Create a Word doc called report.docx with an intro paragraph"

## Template Variables

Skills support Jinja2-style template variables:
- `{{ skill.env.VAR_NAME }}` - Environment variable
- `{{ skill.env.VAR | default: "value" }}` - With default
- `{% for item in list %}...{% endfor %}` - Loops
