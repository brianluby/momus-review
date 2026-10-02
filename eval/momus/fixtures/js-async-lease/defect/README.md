# Leased transport worker
A transport lease must remain active until sendAsync resolves or rejects. The worker owns the lease and releases it after every completed operation, including error outcomes. The async send yields before checking that the lease is still active.
