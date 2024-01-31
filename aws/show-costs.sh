#!/bin/bash
start_date=$(date -u -v-10d "+%Y-%m-%d")
end_date=$(date -u "+%Y-%m-%d")
aws ce get-cost-and-usage-with-resources \
	--granularity MONTHLY \
	--time-period Start=${start_date},End=${end_date} \
	--metrics "UnblendedCost" \
	--group-by '[{"Type": "DIMENSION", "Key": "RESOURCE_ID"}, {"Type": "DIMENSION", "Key": "SERVICE"}]' \
	--filter '{"Dimensions": { "Key": "INSTANCE_TYPE", "Values": [ "UsageQuantity", "BlendedCosts", "RESOURCE_ID" ] }}' \
	--output text --query 'ResultsByTime[].Total.UnblendedCost.{Amount: Amount, Unit: Unit}' | \
cat
