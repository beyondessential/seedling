import { Box } from "@mui/material";
import { parseQuantity } from "../lib/quantity";

/** The amount a `quantity` value resolves to, shown beside the value as it is
 *  entered, or nothing where the value is not a quantity. */
// spec: ACN#presentation-of-kinds
export function QuantityAmount({ value }: { value: string }) {
  if (value === "") return null;
  const parsed = parseQuantity(value);
  if (!("amount" in parsed)) return null;
  return (
    <Box
      component="span"
      sx={{ mx: 1, fontFamily: "monospace", color: "text.secondary", whiteSpace: "nowrap" }}
    >
      = {parsed.amount.toLocaleString()}
    </Box>
  );
}
