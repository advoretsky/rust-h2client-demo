#!/bin/bash
aws ec2 stop-instances --instance-ids $@ --output text --query 'StartingInstances[0].CurrentState.Name' | cat
