#!/bin/bash

while IFS="," read -r module pubkey; do
  anchor idl fetch ${pubkey} > idls/${module}.json 
done << EOF
voter_stake_registry,hvsrNC3NKbcryqDs2DocYHZ9yPKEVzdSjQG6RVtK1s8
helium_entity_manager,hemjuPXBpNvggtaUnN1MwT3wrdhttKEfosTcc2P9Pg8
circuit_breaker,circAbx64bbsscPbQzZAUvuXpHqrCe6fLMzc2uKXz9g
helium_sub_daos,hdaoVTCqhfHHo75XdAMxBKdUqvq1i5bF23sisBqVgGR
price_oracle,porcSnvH9pvcYPmQ65Y8qcZSRxQBiBBQX7UV5nmBegy
fanout,fanqeMu3fw8R4LwKNbahPtYXJsyLL6NXyfe2BqzhfB6
data_credits,credMBJhYFzfn7NxBMdU4aUqFggAjgztaCcv2Fo6fPT
rewards_oracle,rorcfdX4h9m9swCKgcypaHJ8NGYVANBpmV9EHn3cYrF
hexboosting,hexbnKYoA2GercNNhHUCCfrTRWrHjT6ujKPXTa5NPqJ
mobile_entity_manager,memMa1HG4odAFmUbGWfPwS1WWfK95k99F2YTkGvyxZr
lazy_transactions,1atrmQs3eq1N2FEYWu6tyTXbCjP4uQwExpjtnhXtS8h
treasury_management,treaf4wWBBty3fHdyBpo35Mz84M8k3heKXmjmi9vFt5
EOF

anchor idl fetch 1azyuavdMyvsivtNxPoz6SucD18eDHeXzFCUPq5XU7w | \
  jq '.instructions |= map(if .name == "distribute_custom_destination_v0" then .accounts |= map(if .name == "common" then .name = "common_1" else . end) else . end)' | \
  jq '.instructions |= map(if .name == "distribute_rewards_v0" then .accounts |= map(if .name == "common" then .name = "common_2" else . end) else . end)' \
  > /tmp/lazy_distributor.json && \
  mv /tmp/lazy_distributor.json idls/
  
anchor idl fetch cmtDvXumGCrqC1Age74AVPhSRVXJMd8PJS91L8KbNCK | \
  jq '.metadata.address = "cmtDvXumGCrqC1Age74AVPhSRVXJMd8PJS91L8KbNCK"' \
  > /tmp/spl_account_compression.json && \
  anchor idl convert /tmp/spl_account_compression.json \
  > idls/spl_account_compression.json

anchor idl fetch BGUMAp9Gq7iTEuizy4pqaxsTyUCBK68MDfK752saRPUY | \
  jq '.metadata.address = "BGUMAp9Gq7iTEuizy4pqaxsTyUCBK68MDfK752saRPUY"' \
  > /tmp/bubblegum.json && \
  anchor idl convert /tmp/bubblegum.json \
  > idls/bubblegum.json
