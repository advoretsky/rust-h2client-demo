#!/bin/sh
./current-region.sh
aws ec2 describe-instances --query 'Reservations[*].Instances[*].[InstanceId, State.Name, PublicIpAddress, NetworkInterfaces[0].Ipv6Addresses[0].Ipv6Address, CpuOptions.CoreCount, Tags[?Key==`Name`].Value|[0]]' --output text | cat
