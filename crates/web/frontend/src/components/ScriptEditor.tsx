import UploadFileIcon from "@mui/icons-material/UploadFile";
import { Box, Button, Typography, useTheme } from "@mui/material";
import CodeMirror from "@uiw/react-codemirror";
import { useRef, useState } from "react";
import { rhaiLanguage } from "../lib/rhai-lang";

interface Props {
  value: string;
  onChange: (value: string) => void;
  minHeight?: string;
}

export function ScriptEditor({ value, onChange, minHeight = "60vh" }: Props) {
  const { palette: { mode } } = useTheme();
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [fileName, setFileName] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState(false);

  const loadFile = async (file: File) => {
    const text = await file.text();
    onChange(text);
    setFileName(file.name);
  };

  const handleFileChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (file) await loadFile(file);
    e.target.value = "";
  };

  const handleDragOver = (e: React.DragEvent) => {
    if (e.dataTransfer.types.includes("Files")) {
      e.preventDefault();
      setDragOver(true);
    }
  };

  const handleDrop = async (e: React.DragEvent) => {
    setDragOver(false);
    const file = e.dataTransfer.files[0];
    if (!file) return;
    e.preventDefault();
    await loadFile(file);
  };

  return (
    <Box>
      <Box sx={{ display: "flex", alignItems: "center", mb: 0.5, gap: 1 }}>
        <Typography variant="caption" color="text.secondary">
          Script
        </Typography>
        <Box sx={{ flexGrow: 1 }} />
        {fileName && (
          <Typography
            variant="caption"
            color="text.secondary"
            sx={{ fontFamily: "monospace", opacity: 0.7 }}
          >
            {fileName}
          </Typography>
        )}
        <Button
          size="small"
          startIcon={<UploadFileIcon fontSize="small" />}
          onClick={() => fileInputRef.current?.click()}
          sx={{ minWidth: 0 }}
        >
          Load file
        </Button>
        <input
          ref={fileInputRef}
          type="file"
          style={{ display: "none" }}
          onChange={handleFileChange}
        />
      </Box>
      <Box
        onDragOver={handleDragOver}
        onDragLeave={() => setDragOver(false)}
        onDrop={handleDrop}
        sx={{
          border: "1px solid",
          borderColor: dragOver ? "primary.main" : "divider",
          borderRadius: 1,
          overflow: "hidden",
          transition: "border-color 0.15s",
          "& .cm-editor": { minHeight },
          "& .cm-scroller": { fontFamily: "monospace", fontSize: "0.875rem" },
        }}
      >
        <CodeMirror
          value={value}
          onChange={onChange}
          extensions={[rhaiLanguage]}
          theme={mode}
          basicSetup={{
            lineNumbers: true,
            foldGutter: true,
            highlightActiveLine: true,
            highlightSelectionMatches: true,
            closeBrackets: true,
            tabSize: 4,
          }}
        />
      </Box>
    </Box>
  );
}
