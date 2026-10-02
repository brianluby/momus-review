# Invoice service
The checkout caller uses cents. Multiply the total by the basis-point discount and round that discount amount down, then subtract it from the total. For example, 2001 cents at 2500 basis points yields a 500-cent discount and 1501 cents payable. Inputs are any u64 cents and 0..10000 basis points.
