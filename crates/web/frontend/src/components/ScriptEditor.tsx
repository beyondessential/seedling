import UploadFileIcon from "@mui/icons-material/UploadFile";
import { Box, Button, Typography } from "@mui/material";
import { useRef, useState } from "react";

interface Props {
  value: string;
  onChange: (value: string) => void;
  minHeight?: string;
}

export function ScriptEditor({ value, onChange, minHeight = "60vh" }: Props) {
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
      <textarea
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onDragOver={handleDragOver}
        onDragLeave={() => setDragOver(false)}
        onDrop={handleDrop}
        spellCheck={false}
        style={{
          width: "100%",
          minHeight,
          fontFamily: "monospace",
          fontSize: "0.875rem",
          lineHeight: 1.5,
          padding: "12px",
          boxSizing: "border-box",
          resize: "vertical",
          border: `1px solid ${dragOver ? "rgba(144,202,249,0.7)" : "rgba(255,255,255,0.23)"}`,
          borderRadius: "4px",
          background: dragOver ? "rgba(144,202,249,0.05)" : "transparent",
          color: "inherit",
          outline: "none",
          transition: "border-color 0.15s, background 0.15s",
        }}
      />
    </Box>
  );
}
