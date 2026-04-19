import { Typography } from "@mui/material";
import { useParams } from "react-router-dom";

export default function AppDetail() {
  const { name } = useParams<{ name: string }>();
  return <Typography sx={{ p: 2 }}>App: {name}</Typography>;
}
